{
  description = "Home server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    sops-nix.url = "github:Mic92/sops-nix";
    sops-nix.inputs.nixpkgs.follows = "nixpkgs";
    disko.url = "github:nix-community/disko/latest";
    disko.inputs.nixpkgs.follows = "nixpkgs";
    # rust builds in two layers: every dependency once, cached in the
    # bucket until Cargo.lock changes; our crates on top, a minute
    crane.url = "github:ipetkov/crane";
    # a toolchain with the wasm32 target, for the browser side
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      sops-nix,
      disko,
      crane,
      rust-overlay,
      ...
    }:
    let
      # devShells are per-system; nixosConfigurations are not. Both machines
      # here are x86_64-linux, so one system is enough for now.
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      # every vm test, by name: nix/tests/<name>.nix + .py. Also what the ci
      # matrix runs and what main requires (nix/modules/forge/forgejo.nix)
      vmTests = [
        "console"
        "directory"
        "metrics"
        "backup"
        "release"
        "thanos"
        "members-runner"
        "games"
        "forge"
        "transcode"
        "network"
        "library"
        "pgrestore"
        "llm"
      ];
      rust =
        let
          craneLib = crane.mkLib pkgs;
          # One source tree per binary: the workspace files, every member's
          # manifest (cargo loads the whole workspace), and the full trees
          # of only the crates that binary is built from - with the pages'
          # templates, stylesheets, scripts and icons those include. The
          # members it does not use get an empty stub target, so a change
          # to the verifier's pages is not a games rebuild, not a games vm
          # test, not a release of anything but the verifier. The filter
          # step is content-addressed; the stub step only sees its output.
          srcFor =
            name: dirs:
            let
              filtered = pkgs.lib.cleanSourceWith {
                src = ./.;
                filter =
                  path: type:
                  let
                    rel = pkgs.lib.removePrefix (toString ./. + "/") (toString path);
                    top = builtins.elem rel [
                      "Cargo.toml"
                      "Cargo.lock"
                    ];
                    # the trees on the way to every member, and every manifest
                    tree =
                      builtins.elem rel [
                        "box"
                        "client"
                        "app"
                      ]
                      || builtins.match "(box|client)/[^/]+" rel != null;
                    manifest = builtins.match "(box|client)/[^/]+/Cargo\\.toml" rel != null || rel == "app/Cargo.toml";
                    # the app's pages, icons and tauri config sit beside its src
                    appFile = pkgs.lib.hasPrefix "app/" rel;
                    ours = builtins.any (d: rel == d || pkgs.lib.hasPrefix "${d}/" rel) dirs;
                  in
                  top
                  || manifest
                  || (type == "directory" && tree)
                  || (
                    ours
                    && (
                      type == "directory"
                      || craneLib.filterCargoSources path type
                      || builtins.match ".*/templates/.*" rel != null
                      # the pages, scripts and styles: the gate reads its own
                      # from the pages directory at start (pages below), so
                      # they are not its source; other crates still compile
                      # theirs in, and the tests read them from the tree
                      || (name != "verify" && builtins.match ".*/web/.*" rel != null)
                      # where askama looks for templates beyond the crate's own
                      || builtins.match ".*/askama\\.toml" rel != null
                      || appFile
                    )
                  );
              };
            in
            pkgs.runCommand "src-${name}" { } ''
              cp -r ${filtered} $out
              chmod -R u+w $out
              for m in $out/box/* $out/client/* $out/app; do
                [ -f "$m/Cargo.toml" ] || continue
                if [ ! -d "$m/src" ]; then
                  mkdir -p "$m/src"
                  : > "$m/src/lib.rs"
                  echo 'fn main() {}' > "$m/src/main.rs"
                fi
              done
            '';
          # every crate: the checks (fmt, clippy, tests) build the workspace
          src = srcFor "workspace" [
            "box"
            "client"
            "app"
          ];
          # the crates behind each binary: the path dependencies in the
          # manifests, followed to the end, so a list never goes stale
          crateDeps =
            dir:
            let
              m = builtins.fromTOML (builtins.readFile (./. + "/${dir}/Cargo.toml"));
              deps = (m.dependencies or { }) // (m.build-dependencies or { });
              norm =
                base: rel:
                let
                  parts = pkgs.lib.filter (p: p != "" && p != ".") (pkgs.lib.splitString "/" "${base}/${rel}");
                  walk =
                    acc: ps:
                    if ps == [ ] then
                      acc
                    else if builtins.head ps == ".." then
                      walk (pkgs.lib.init acc) (builtins.tail ps)
                    else
                      walk (acc ++ [ (builtins.head ps) ]) (builtins.tail ps);
                in
                pkgs.lib.concatStringsSep "/" (walk [ ] parts);
            in
            pkgs.lib.unique (
              map (d: norm dir d.path) (pkgs.lib.filter (d: d ? path) (builtins.attrValues deps))
            );
          crateClosure =
            roots:
            let
              go =
                seen: todo:
                if todo == [ ] then
                  seen
                else
                  let
                    d = builtins.head todo;
                    rest = builtins.tail todo;
                  in
                  if builtins.elem d seen then go seen rest else go (seen ++ [ d ]) (rest ++ crateDeps d);
            in
            go [ ] roots;
          crateSrc = name: roots: srcFor name (crateClosure roots);
          # What the gate serves as it is - pages, scripts, styles, fonts,
          # icons - in a directory laid out like the tree, apart from the
          # gate itself: a page changing rebuilds this and nothing else.
          pages = pkgs.lib.cleanSourceWith {
            name = "dd-pages";
            src = ./.;
            filter =
              path: type:
              let
                rel = pkgs.lib.removePrefix (toString ./. + "/") (toString path);
              in
              (
                type == "directory"
                && (
                  rel == "box"
                  || builtins.match "box/[^/]+" rel != null
                  || builtins.match "box/[^/]+/web(/.*)?" rel != null
                )
              )
              || builtins.match "box/[^/]+/web/.*" rel != null;
          };
          # What a dependency build sees: the lockfile and the manifests, with
          # crane's stand-in for every target, and nothing else. Each
          # dependency cache is keyed on this, so a page, a template, a patch
          # or our own code changing leaves every one of them as it is; only
          # the lockfile or a manifest moving rebuilds them. Before, each was
          # made from its binary's whole tree and a page edit recompiled
          # every dependency (CI history: page-only runs as slow as
          # lockfile ones).
          depsSrc = craneLib.mkDummySrc {
            src = pkgs.lib.cleanSourceWith {
              src = ./.;
              filter =
                path: type:
                let
                  rel = pkgs.lib.removePrefix (toString ./. + "/") (toString path);
                in
                builtins.elem rel [
                  "Cargo.toml"
                  "Cargo.lock"
                  "app/Cargo.toml"
                ]
                || (
                  type == "directory"
                  && (
                    builtins.elem rel [
                      "box"
                      "client"
                      "app"
                    ]
                    || builtins.match "(box|client)/[^/]+" rel != null
                  )
                )
                || builtins.match "(box|client)/[^/]+/Cargo\\.toml" rel != null;
            };
          };
          sources = {
            dd = crateSrc "dd" [
              "client/cli"
              "client/gitremote"
            ];
            agent = crateSrc "agent" [ "box/release" ];
            # the gate serves the services' pages, which live with each
            # service (box/<service>/web, templates), not in a crate
            verify = srcFor "verify" (
              crateClosure [ "box/verify" ]
              ++ [
                "box/web"
                "box/photos"
                "box/media"
                "box/files"
                "box/fleet"
                "box/chat"
              ]
            );
            games = crateSrc "games" [ "box/games" ];
            transcode = crateSrc "transcode" [ "box/transcode" ];
            web = crateSrc "web" [ "client/web" ];
            app = crateSrc "app" [ "app" ];
          };
          # the app's network engine: Tailscale's tsnet behind four C calls
          # (app/net), a static archive the Rust core links. The one Go in
          # the tree.
          appNet = pkgs.buildGoModule {
            pname = "commonty-net";
            version = "0.1.0";
            src = ./app/net;
            vendorHash = "sha256-n6mcL9euSTEeP2l4gg8A3uMTK2xrIh7xgeuVCsESRFU=";
            buildPhase = ''
              runHook preBuild
              go build -buildmode=c-archive -o libcommontynet.a .
              runHook postBuild
            '';
            installPhase = ''
              mkdir -p $out/lib $out/include
              cp libcommontynet.a $out/lib/
              cp libcommontynet.h $out/include/
            '';
            doCheck = false;
          };
          # the same engine for Android: a shared library per abi (Go makes
          # no archives there), cross-built with the ndk. `nix build
          # .#net-android`, then the .so files go into the apk (README).
          androidPkgs = import nixpkgs {
            inherit system;
            config = {
              allowUnfree = true;
              android_sdk.accept_license = true;
            };
          };
          androidSdk =
            (androidPkgs.androidenv.composeAndroidPackages {
              platformVersions = [
                "34"
                "36"
              ];
              buildToolsVersions = [
                "34.0.0"
                "35.0.0"
              ];
              includeNDK = true;
              ndkVersions = [ "26.1.10909125" ];
              includeEmulator = true;
              includeSystemImages = true;
              systemImageTypes = [ "google_apis" ];
              abiVersions = [ "x86_64" ];
              cmdLineToolsVersion = "11.0";
            }).androidsdk;
          androidHome = "${androidSdk}/libexec/android-sdk";
          androidNdk = "${androidHome}/ndk/26.1.10909125";
          appNetAndroid = pkgs.buildGoModule {
            pname = "commonty-net-android";
            version = "0.1.0";
            src = ./app/net;
            vendorHash = "sha256-n6mcL9euSTEeP2l4gg8A3uMTK2xrIh7xgeuVCsESRFU=";
            buildPhase = ''
              runHook preBuild
              T=${androidNdk}/toolchains/llvm/prebuilt/linux-x86_64/bin
              export CGO_ENABLED=1 GOOS=android
              mkdir -p $out/arm64-v8a $out/x86_64
              GOARCH=arm64 CC=$T/aarch64-linux-android34-clang go build -buildmode=c-shared -o $out/arm64-v8a/libcommontynet.so .
              GOARCH=amd64 CC=$T/x86_64-linux-android34-clang go build -buildmode=c-shared -o $out/x86_64/libcommontynet.so .
              runHook postBuild
            '';
            installPhase = "true";
            doCheck = false;
          };
          # what the app's window is made of; the workspace checks compile
          # the app too, so they need it as well
          appLibs = [
            pkgs.webkitgtk_4_1
            pkgs.gtk3
            pkgs.libsoup_3
            pkgs.glib
            pkgs.openssl
          ];
          # the same toolchain, plus the wasm32 target; nixpkgs' rustc
          # ships no std for it
          wasmToolchain =
            (pkgs.extend rust-overlay.overlays.default).rust-bin.stable.latest.minimal.override
              {
                targets = [ "wasm32-unknown-unknown" ];
              };
          craneWasm = craneLib.overrideToolchain wasmToolchain;
          wasmCommon = {
            src = sources.web;
            strictDeps = true;
            doCheck = false;
            CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
            cargoExtraArgs = "-p dd-web";
            # getrandom 0.3 takes its backend from a cfg, not a feature
            RUSTFLAGS = "--cfg getrandom_backend=\"wasm_js\"";
          };
          wasmArtifacts = craneWasm.buildDepsOnly (
            wasmCommon
            // {
              pname = "dd-web-deps";
              dummySrc = depsSrc;
              version = "0.1.0";
            }
          );
          wasmBuild = craneWasm.buildPackage (
            wasmCommon
            // {
              pname = "dd-web";
              version = "0.1.0";
              cargoArtifacts = wasmArtifacts;
            }
          );
          # What every rust build shares. The app's window libraries and its
          # network engine are the app's alone: in here they made every
          # package's dependency build depend on them, so a change to the Go
          # engine recompiled the boxes' dependencies from nothing.
          common = {
            inherit src;
            strictDeps = true;
            nativeBuildInputs = [ pkgs.pkg-config ];
            # the cli's mount (dd media) links libfuse
            buildInputs = [ pkgs.fuse3 ];
            doCheck = false;
          };
          # the app: its window, and its network engine (app/build.rs links
          # it from here)
          appCommon = common // {
            buildInputs = [ pkgs.fuse3 ] ++ appLibs;
            COMMONTY_NET_LIB_DIR = "${appNet}/lib";
          };
          # The workspace but the app, for the checks (clippy, tests): the
          # app's dependency tree (tauri, webkit) is most of the compile,
          # and nothing else in the workspace uses it. The app is checked
          # on its own (checks.clippy-app), only when it changed.
          workspaceArgs = "--locked --workspace --exclude commonty";
          # The tests build unoptimised and without debug info: a release
          # build of every dependency cost most of the test job, only to run
          # them once. Only the tests use this cache; binaries stay release.
          testProfile = {
            CARGO_PROFILE = "dev";
            CARGO_PROFILE_DEV_DEBUG = "false";
          };
          cargoArtifacts = craneLib.buildDepsOnly (
            common
            // {
              pname = "dd-deps";
              dummySrc = depsSrc;
              version = "0.1.0";
              cargoExtraArgs = workspaceArgs;
              inherit (testProfile) CARGO_PROFILE CARGO_PROFILE_DEV_DEBUG;
            }
          );
          # and one dependency build per binary, with that binary's own
          # `-p`: cargo unifies features per package set, so a cache built
          # for the whole workspace does not match `-p dd` and every run
          # recompiled rustic and its friends only to throw them away
          crateWith =
            base: pname: source: cargoExtraArgs:
            let
              c = base // {
                src = source;
              };
            in
            craneLib.buildPackage (
              c
              // {
                inherit pname cargoExtraArgs;
                version = "0.1.0";
                cargoArtifacts = craneLib.buildDepsOnly (
                  c
                  // {
                    inherit cargoExtraArgs;
                    pname = "${pname}-deps";
                    dummySrc = depsSrc;
                    version = "0.1.0";
                  }
                );
              }
            );
          crate = crateWith common;
          # the app's own dependency build, shared by its package and its check
          appArtifacts = craneLib.buildDepsOnly (
            appCommon
            // {
              src = sources.app;
              pname = "commonty-deps";
              dummySrc = depsSrc;
              version = "0.1.0";
              cargoExtraArgs = "-p commonty";
            }
          );
        in
        {
          # a package must be a derivation (flake check), not a source path
          pages = pkgs.runCommandLocal "dd-pages" { } "cp -r ${pages} $out";
          # the cli, and `git remote add origin dd::...`, which dd repo calls too
          # `dd media` mounts libraries with rclone; the binary knows where it is
          dd = (crate "dd" sources.dd "-p dd -p git-remote-dd").overrideAttrs (old: {
            nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.makeWrapper ];
            postFixup =
              (old.postFixup or "")
              + "\n"
              + ''
                wrapProgram $out/bin/dd --set DD_RCLONE ${pkgs.rclone}/bin/rclone
              '';
          });
          # The release agent, on every box. Server side like verify: the
          # release crate holds the file format the cli signs and this
          # binary checks, and nothing that needs a keyring.
          agent = crate "dd-agent" sources.agent "-p release";
          # The verifier behind nginx's auth_request. Built separately from dd
          # rather than as another binary in the same derivation: this one
          # runs on a server and has no business pulling in the keyring/dbus
          # stack that the cli needs.
          verify = crate "verify" sources.verify "-p verify";
          # the manager behind the Games tile (modules/games.nix)
          games = crate "dd-games" sources.games "-p games";
          # one file, one viewer, in memory: the compute side of the
          # encrypted libraries (modules/library/transcode.nix)
          transcode = crate "dd-transcode" sources.transcode "-p transcode";
          # The app: the same crates as dd behind a window (app/). Wrapped
          # so the webview finds its schemas and gio modules; the dmabuf
          # renderer is off because on nvidia it draws a blank window.
          app = (crateWith appCommon "commonty" sources.app "-p commonty").overrideAttrs (old: {
            nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.wrapGAppsHook3 ];
            # glib-networking for https; GStreamer is how WebKit plays media,
            # and without its plugins nothing played in the app ("appsink not
            # found"): base and good for the pipeline, bad for the streaming
            # (MSE) that the players use, libav to decode H.264 and AAC
            buildInputs = (old.buildInputs or [ ]) ++ [
              pkgs.glib-networking
              pkgs.gst_all_1.gstreamer
              pkgs.gst_all_1.gst-plugins-base
              pkgs.gst_all_1.gst-plugins-good
              pkgs.gst_all_1.gst-plugins-bad
              pkgs.gst_all_1.gst-libav
            ];
            postFixup =
              (old.postFixup or "")
              + "\n"
              + ''
                wrapProgram $out/bin/commonty \
                  --set WEBKIT_DISABLE_DMABUF_RENDERER 1
              '';
          });
          # the app's network engine (app/net): `nix build .#net` for the archive
          net = appNet;
          # the engine's bridge on its own, for a box's stock tailscaled
          netbridge = pkgs.buildGoModule {
            pname = "commonty-net-bridge";
            version = "0.1.0";
            src = ./app/net;
            inherit (appNet) vendorHash;
            subPackages = [ "cmd/netbridge" ];
            env.CGO_ENABLED = "0";
          };
          # and for Android, one .so per abi
          net-android = appNetAndroid;
          # the Android toolchain, for the dev shell below
          android-sdk = androidSdk;
          # Our Rust in the browser: the ente account for a person whose key
          # is a passkey (crates/web). The verifier serves this directory.
          # A film is HLS, because that is what the box's transcode makes
          # and what lets a viewer seek. Safari plays a playlist by itself;
          # nothing else does, so the page needs a player. Pinned by hash
          # and served from this directory beside our own wasm: no
          # third-party blob in the tree, and the page fetches it from the
          # box like everything else it loads.
          web = pkgs.runCommand "dd-web-dist" { nativeBuildInputs = [ pkgs.wasm-bindgen-cli ]; } ''
            mkdir -p $out
            wasm-bindgen --target web --no-typescript --out-dir $out ${wasmBuild}/lib/dd_web.wasm
            cp ${
              pkgs.fetchurl {
                url = "https://cdnjs.cloudflare.com/ajax/libs/hls.js/1.6.5/hls.min.js";
                hash = "sha256-k36IEw6HrUntpsfxCOk+QsInVYxti01bU9205DB6Msw=";
              }
            } $out/hls.js
            # Chat's answers are markdown: turned into html, then cleaned of
            # anything that could run, before the page shows them
            cp ${
              pkgs.fetchurl {
                url = "https://cdnjs.cloudflare.com/ajax/libs/marked/18.0.14/lib/marked.esm.min.js";
                hash = "sha256-brZkvp2IUr5XYSw1XbTW18R/HjM/LClHqIFOEuOxLUY=";
              }
            } $out/marked.js
            cp ${
              pkgs.fetchurl {
                url = "https://cdnjs.cloudflare.com/ajax/libs/dompurify/3.4.16/purify.min.js";
                hash = "sha256-LJCptG1kY/JgOKKbaG6CvJHeAf2snVIp58/js2ATTqI=";
              }
            } $out/purify.js
          '';

          # the checks, on the same compiled artifacts as the binaries: fmt
          # and clippy in seconds, the tests once, all cached by content and
          # shared between the runner boxes through the bucket. ci builds
          # these instead of running cargo a second and third time.
          fmt = craneLib.cargoFmt {
            inherit src;
            pname = "dd";
            version = "0.1.0";
          };
          clippy = craneLib.cargoClippy (
            common
            // {
              # Its own dependencies, checked and never built: clippy needs
              # only their metadata. Sharing the tests' cache made lint wait
              # on - or, cold, repeat - a full release and test build of
              # every dependency, three compiles where one check will do.
              cargoArtifacts = craneLib.buildDepsOnly (
                common
                // {
                  pname = "dd-clippy-deps";
                  dummySrc = depsSrc;
                  version = "0.1.0";
                  cargoExtraArgs = workspaceArgs;
                  cargoBuildCommand = "true";
                  doCheck = false;
                }
              );
              pname = "dd";
              version = "0.1.0";
              cargoExtraArgs = workspaceArgs;
              cargoClippyExtraArgs = "--all-targets -- -D warnings";
            }
          );
          clippy-app = craneLib.cargoClippy (
            appCommon
            // {
              src = sources.app;
              cargoArtifacts = appArtifacts;
              pname = "commonty";
              version = "0.1.0";
              cargoExtraArgs = "-p commonty";
              cargoClippyExtraArgs = "--all-targets -- -D warnings";
            }
          );
          tests = craneLib.cargoTest (
            common
            // {
              inherit cargoArtifacts;
              inherit (testProfile) CARGO_PROFILE CARGO_PROFILE_DEV_DEBUG;
              # common turns checks off for the packages; from 2026-09-23
              # until this line the tests check inherited that and ran none
              doCheck = true;
              # repo.rs runs git with git-remote-dd, another package's binary
              # that cargo test never rebuilds: without this it found the
              # dependency build's stand-in
              preCheck = "cargoWithProfile build ${workspaceArgs}";
              pname = "dd";
              version = "0.1.0";
              cargoExtraArgs = workspaceArgs;
              # the e2e tests spawn verifiers on localhost and run git; the
              # library's format is checked against rclone itself; a box's
              # attestation is an ssh signature
              nativeBuildInputs = [
                pkgs.pkg-config
                pkgs.gitMinimal
                pkgs.openssh
              ];
              RCLONE = "${pkgs.rclone}/bin/rclone";
            }
          );
        };
    in
    {
      # `nix develop` drops you into a shell with the rust toolchain on PATH.
      # Nothing is installed globally and any machine cloning this repo gets
      # exactly these versions.
      devShells.${system} = {
        default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.rust-analyzer # editor: completion, jump to definition
            pkgs.clippy # linter that teaches you the language
            pkgs.rustfmt
            pkgs.pkg-config # crates with C dependencies need this to find them
            pkgs.fuse3 # the cli's mount
            pkgs.sops # `dd secret run -- ...` runs this
            pkgs.age
            pkgs.cargo-tauri # `cargo tauri dev` in app/
          ]
          ++ [
            # the app's window, for `cargo build -p commonty` here
            pkgs.webkitgtk_4_1
            pkgs.gtk3
            pkgs.libsoup_3
            pkgs.glib
            pkgs.openssl
            pkgs.glib-networking
          ];
          RUST_BACKTRACE = "1";
          # the app's network engine, for `cargo build -p commonty` here
          COMMONTY_NET_LIB_DIR = "${rust.net}/lib";
          # the webview finds its gio modules (tls) and, on nvidia, draws
          WEBKIT_DISABLE_DMABUF_RENDERER = "1";
          GIO_MODULE_DIR = "${pkgs.glib-networking}/lib/gio/modules";
          # and plays video: webkit decodes through gstreamer's plugins
          GST_PLUGIN_SYSTEM_PATH_1_0 = pkgs.lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" (
            with pkgs.gst_all_1;
            [
              gstreamer
              gst-plugins-base
              gst-plugins-good
              gst-plugins-bad
              gst-libav
            ]
          );
        };

        # `nix develop .#android`: the Android toolchain for the app. From
        # app/: `cargo tauri android build --apk` with COMMONTY_NET_LIB_DIR
        # pointing at the jniLibs the engine's .so files were copied into
        # (app/README.md). Not in ci: a phone build is made here.
        android =
          let
            androidRust = (pkgs.extend rust-overlay.overlays.default).rust-bin.stable.latest.default.override {
              targets = [
                "aarch64-linux-android"
                "x86_64-linux-android"
              ];
            };
          in
          pkgs.mkShell {
            packages = [
              rust.android-sdk
              androidRust
              pkgs.cargo-tauri
              pkgs.jdk17
              pkgs.gradle
              pkgs.go
              pkgs.pkg-config
              pkgs.nodejs
            ];
            ANDROID_HOME = "${rust.android-sdk}/libexec/android-sdk";
            ANDROID_SDK_ROOT = "${rust.android-sdk}/libexec/android-sdk";
            NDK_HOME = "${rust.android-sdk}/libexec/android-sdk/ndk/26.1.10909125";
            JAVA_HOME = pkgs.jdk17;
            # gradle's aapt2 must be the sdk's: the one it downloads is not for nix
            GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${rust.android-sdk}/libexec/android-sdk/build-tools/35.0.0/aapt2";
            COMMONTY_NET_ANDROID = "${rust.net-android}";
          };
      };

      # `nix build .#dd` / `nix run .#dd -- status`. Every dependency is
      # fetched by hash from Cargo.lock, so the binary is as reproducible as
      # the nixos closure. crane builds the dependencies as their own
      # derivation, so a change to our code costs our code's compile, not
      # the three hundred crates under it. The tests ran already in ci's
      # rust job on this same source; no second run in here.
      packages.${system} =
        builtins.removeAttrs rust [
          "fmt"
          "clippy"
          "tests"
          "android-sdk"
        ]
        // {
          # `dd secret run` falls back to this when sops is not on PATH. The
          # fleet's age key goes into that process's environment, so it is
          # this repo's pinned nixpkgs and not whatever unstable is serving
          # at the moment of use.
          inherit (pkgs) sops;
          # `dd release app` checks a build's provenance with this; the
          # distribution's gh is often too old to have `attestation`
          inherit (pkgs) gh;
        };

      # Boxes booted as vms and driven through the failure cases, so the
      # modules the real hosts import are proven before a host sees them.
      # `nix build .#checks.x86_64-linux.directory`; ci runs them when the
      # modules or the verifier change. Not part of a plain `nix flake check`
      # run's build set on a laptop without kvm: `--no-build` there.
      checks.${system} =
        let
          args = {
            inherit pkgs self;
            lib = nixpkgs.lib;
          };
          vm =
            name:
            (import ./nix/lib/vm-test.nix {
              inherit pkgs vmTests;
              lib = nixpkgs.lib;
              boxNames = builtins.attrNames (builtins.fromJSON (builtins.readFile ./fleet/boxes.json));
            })
              name
              (import ./nix/tests/${name}.nix args);
        in
        {
        }
        // nixpkgs.lib.genAttrs vmTests vm
        // {
          inherit (rust)
            fmt
            clippy
            clippy-app
            tests
            ;
          placement = import ./nix/tests/placement.nix args;
          ci = import ./nix/tests/ci.nix (args // { inherit vmTests; });
          boxes = import ./nix/tests/boxes.nix args;
          # the app carries the site's signed-in pages, their scripts, the
          # stylesheet and the fonts at the site's paths (app/carried
          # lists them, scripts/sync-app-web copies them); two copies, kept
          # one here
          stylesheet =
            let
              lines = builtins.filter (l: l != "" && builtins.substring 0 1 l != "#") (
                nixpkgs.lib.splitString "\n" (builtins.readFile ./app/carried)
              );
              pairs = map (l: builtins.filter (w: w != "") (nixpkgs.lib.splitString " " l)) lines;
            in
            pkgs.runCommand "one-set-of-pages" { } (
              nixpkgs.lib.concatMapStrings (p: ''
                cmp ${./. + "/${builtins.elemAt p 0}"} ${./app/web/_dd + "/${builtins.elemAt p 1}"}
              '') pairs
              + ''
                # nothing carried that the list does not name
                test "$(find ${./app/web/_dd} -type f | wc -l)" = "${toString (builtins.length pairs)}"
                touch $out
              ''
            );
        };

      # One box per entry in fleet/boxes.json: its hardware file plus its
      # roles. What a box runs is data, not a hand-written import list, and
      # the per-box wiring (who its peers are, which prometheus grafana reads)
      # is derived from the same list.
      nixosConfigurations =
        let
          boxes = builtins.fromJSON (builtins.readFile ./fleet/boxes.json);
          lib = nixpkgs.lib;
          directoryOf =
            name: box:
            if box.public then
              "https://files.commonty.org/_dd/directory"
            else
              "http://${box.tailnet}:4181/_dd/directory";
          # a role that declares secrets imports roles/_sops.nix; the sops
          # module has to be present for it. Roles are files; look inside.
          needsSops =
            roles: builtins.any (r: lib.hasInfix "_sops.nix" (builtins.readFile ./nix/roles/${r}.nix)) roles;
          storage = lib.filterAttrs (_: b: builtins.elem "storage" b.roles) boxes;
          # the garage cluster: every storage box, each told about the others
          # whose ids are known (a box's id exists once it has started once)
          garageOf = name: box: {
            dd.garage = {
              zone = box.regionId;
              inherit (box.garage) capacity dataDir;
              publicAddr = "${box.tailnet}:3901";
              peers = lib.mapAttrsToList (_: b: "${b.garage.id}@${b.tailnet}:3901") (
                lib.filterAttrs (n: b: n != name && b.garage ? id) storage
              );
            };
          };
          # closures come from the nix-cache bucket in the garage cluster:
          # through the box's own garage, or a storage box's over the tailnet
          cacheOf =
            name: box:
            let
              host =
                if builtins.elem "storage" box.roles then
                  "127.0.0.1"
                else
                  (builtins.head (builtins.attrValues storage)).tailnet;
            in
            {
              dd.agent.cache = "s3://nix-cache?endpoint=${host}:3900&scheme=http&region=us-east-1";
            };
          # a box without garage backs up to a storage box over the tailnet
          backupEndpoint =
            name: box:
            lib.optionalAttrs (!(builtins.elem "storage" box.roles)) {
              dd.backup.endpoint = "http://${(builtins.head (builtins.attrValues storage)).tailnet}:3900";
            };
          mkBox =
            name: box:
            lib.nixosSystem {
              specialArgs = {
                inherit self vmTests;
                boxNames = builtins.attrNames boxes;
                # a unit's script from a file beside its module (nix/lib/script.nix)
                ddScript = import ./nix/lib/script.nix { lib = nixpkgs.lib; };
              };
              modules = [
                ./nix/hosts/${name}/hardware.nix
                {
                  networking.hostName = name;
                  dd.box.tailnet = box.tailnet;
                  dd.box.site = box.siteId;
                  dd.box.region = box.regionId;
                  dd.verify.peers = lib.mapAttrsToList directoryOf (lib.filterAttrs (n: _: n != name) boxes);
                  # itself included: the Boxes page is the whole fleet
                  dd.verify.fleet = lib.mapAttrs (_: b: b.tailnet) boxes;
                  # the other boxes, as the agent checks a release did not
                  # cost this box its way off itself
                  dd.agent.reach = lib.mapAttrsToList (_: b: "${b.tailnet}:22") (
                    lib.filterAttrs (n: _: n != name) boxes
                  );
                  # the control box's tailnet address: boxes pin the control
                  # server's name to it (modules/net/box.nix)
                  dd.net.controlAddress = (lib.findFirst (b: b.public) box (builtins.attrValues boxes)).tailnet;
                }
              ]
              ++ map (r: ./nix/roles/${r}.nix) box.roles
              ++ lib.optional (builtins.elem "storage" box.roles) (garageOf name box)
              ++ [
                (backupEndpoint name box)
                (cacheOf name box)
              ]
              ++ lib.optional (needsSops box.roles) sops-nix.nixosModules.sops
              ++ lib.optional (builtins.pathExists ./nix/hosts/${name}/disko.nix) disko.nixosModules.disko
              ++ lib.optional (builtins.elem "observe" box.roles) {
                # every box's sidecar, for the fleet-wide query
                dd.thanos.sidecars = lib.mapAttrsToList (
                  n: b: if n == name then "127.0.0.1:10901" else "${b.tailnet}:10901"
                ) boxes;
              };
            };
        in
        lib.mapAttrs mkBox boxes;
    };
}
