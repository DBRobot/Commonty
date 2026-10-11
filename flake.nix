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
    # the app drawn by the machine's own NVIDIA driver, off NixOS
    nix-gl-host.url = "github:numtide/nix-gl-host";
    nix-gl-host.inputs.nixpkgs.follows = "nixpkgs";
    # Secure Boot: the boxes sign what they boot, so their TPMs unlock the
    # disks only for software signed here (nix/modules/box/secure-boot.nix)
    lanzaboote.url = "github:nix-community/lanzaboote/v1.2.0";
    lanzaboote.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      sops-nix,
      disko,
      crane,
      rust-overlay,
      nix-gl-host,
      lanzaboote,
      ...
    }:
    let
      # devShells are per-system; nixosConfigurations are not. Both machines
      # here are x86_64-linux, so one system is enough for now.
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      # every vm test, by name: nix/tests/<name>.nix + .py. Also what the ci
      # matrix runs (nix/tests/ci.nix holds the two to one list)
      vmTests = [
        "acme"
        "console"
        "directory"
        "metrics"
        "metrics-ui"
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
        "photos"
        "llm"
        "vault"
        "adblock"
        "unlock"
        "leaving"
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
                        "servers"
                        "client"
                        "app"
                      ]
                      || builtins.match "(servers/system|servers|client)/[^/]+" rel != null;
                    manifest =
                      builtins.match "(servers/system|servers|client)/[^/]+/Cargo\\.toml" rel != null
                      || rel == "app/Cargo.toml";
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
                      # a test's fixtures beside it (client/mail/tests)
                      || builtins.match ".*/tests/.*\\.json" rel != null
                      # the pages, scripts and styles: the gate reads its own
                      # from the pages directory at start (pages below), so
                      # they are not its source; other crates still compile
                      # theirs in, and the tests read them from the tree
                      || (name != "verify" && builtins.match ".*/web(-shared)?/.*" rel != null)
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
              for m in $out/servers/* $out/servers/system/* $out/client/* $out/app; do
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
            "servers"
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
              # servers/web-shared (the shared pages) and servers/<service>/web, whole
              rel == "servers"
              || (type == "directory" && builtins.match "servers/[^/]+" rel != null)
              || builtins.match "servers/([^/]+/)?web(/.*)?" rel != null
              || builtins.match "servers/web-shared(/.*)?" rel != null
              # the docs, which the gate serves as they are (servers/gate/src/docs.rs)
              || builtins.match "docs(/.*)?" rel != null;
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
                      "servers"
                      "client"
                      "app"
                    ]
                    || builtins.match "(servers/system|servers|client)/[^/]+" rel != null
                  )
                )
                || builtins.match "(servers/system|servers|client)/[^/]+/Cargo\\.toml" rel != null;
            };
          };
          # The same, under the name of the source a crate builds from: a
          # build script may record where it wrote (tauri does), and a cache
          # made in "source" pointed the app's build in src-app at nothing
          depsIn = src: pkgs.runCommandLocal src.name { } "cp -r ${depsSrc} $out";
          # The programs that run on the boxes, and dd, one derivation per
          # crate (Cargo.nix, made by crate2nix from Cargo.lock): a
          # dependency moving rebuilds that crate and what uses it, where
          # crane rebuilt every dependency at once. After changing
          # Cargo.lock: nix run nixpkgs#crate2nix -- generate
          cargoNix = pkgs.callPackage ./Cargo.nix {
            inherit buildRustCrateForPkgs;
            defaultCrateOverrides = crateOverrides;
          };
          # cargo's release default: sixteen pieces of each crate compiled
          # at once. buildRustCrate's is one, which made the first build of
          # every crate - all ~800 of them - several times slower.
          buildRustCrateForPkgs = pkgs: pkgs.buildRustCrate.override { defaultCodegenUnits = 16; };
          crateOverrides = pkgs.defaultCrateOverrides // {
            # askama finds the Photos pages' templates beside the gate's
            verify = _: {
              postUnpack = "mkdir -p $NIX_BUILD_TOP/photos && cp -r ${./servers/photos/templates} $NIX_BUILD_TOP/photos/templates";
            };
            # dd's tests: e2e.rs reads the mail Worker's tester from beside
            # the cli, and repo.rs runs git with git-remote-dd from beside
            # dd, naming cargo only to build one that is not there. An
            # override cannot tell a test build from the boxes' (buildTests
            # is set after it), so the copy waits for the test binaries
            dd = _: {
              postUnpack = "mkdir -p $NIX_BUILD_TOP/mail && cp -r ${./client/mail/tests} $NIX_BUILD_TOP/mail/tests";
              postInstall = ''
                if [ -d $out/tests ]; then
                  cp ${perCrate "git-remote-dd"}/bin/git-remote-dd $out/bin/
                fi
              '';
              CARGO = "cargo";
            };
            # fl! wrote a message's arguments in hash-map order, so age's
            # library differed on every build and a box rebuilding dd to
            # vouch for it never matched what CI had built
            i18n-embed-fl = _: { patches = [ ./nix/patches/i18n-embed-fl-args-in-order.patch ]; };
          };
          perCrate = n: cargoNix.workspaceMembers.${n}.build;
          # One crate's tests, built and run from the very builds the boxes'
          # programs use. A crate's features are what the program that
          # carries it asks for; tested on its own it resolved its own, and
          # every dependency it shares came out a different build (660 of
          # them, PR 264). So each crate is tested inside the first box
          # program whose graph holds it, against that program's own
          # dependency builds, cached already: a crate whose code and
          # dependencies did not change is a cache hit, its tests too. A
          # crate with dev-dependencies is tested as its own root, which is
          # the only way they come in (dd is a box program itself; the mail
          # Worker's crate is only ever built for its tests here).
          testHosts = [
            "dd"
            "verify"
            "release"
            "games"
            "transcode"
            "git-remote-dd"
          ];
          graphOf =
            root: runTests: overrides:
            (cargoNix.internal.builtRustCratesWithFeatures {
              packageId = root;
              features = [ "default" ];
              inherit runTests;
              buildRustCrateForPkgsFunc =
                p: (buildRustCrateForPkgs p).override { defaultCrateOverrides = overrides; };
            }).crates;
          hostGraphs = pkgs.lib.genAttrs testHosts (h: graphOf h false crateOverrides);
          # Tests compile unoptimised, as cargo test does: a test program
          # never ships, and optimising it made a change to the gate cost
          # ten minutes (its tests and dd's, each rebuilt at opt-level 3).
          # The boxes' own builds stay optimised and shared; only what is
          # built for the tests alone - each crate's test build, and our own
          # crates in a graph made for dev-dependencies - is not.
          unoptimised = o: {
            extraRustcOpts = (o.extraRustcOpts or [ ]) ++ [
              "-C"
              "opt-level=0"
              "-C"
              "debuginfo=0"
            ];
          };
          testOverrides =
            crateOverrides
            // pkgs.lib.genAttrs (builtins.attrNames cargoNix.workspaceMembers) (
              w: attrs: (crateOverrides.${w} or (_: { })) attrs // unoptimised attrs
            );
          withDevDeps = n: (cargoNix.internal.crates.${n}.devDependencies or [ ]) != [ ];
          # the graph a crate's tests are built in
          testGraph =
            n:
            if withDevDeps n then
              graphOf n true testOverrides
            else
              let
                # a graph lists every crate; the program's own build says which
                # of them it is really built from
                holds =
                  h:
                  h == n
                  || builtins.any (d: d.drvPath == hostGraphs.${h}.${n}.drvPath) hostGraphs.${h}.${h}.completeDeps;
                host = pkgs.lib.findFirst holds null testHosts;
              in
              if host == null then graphOf n true testOverrides else hostGraphs.${host};
          # Lint with the tests: clippy stands in for rustc where a crate of
          # ours is compiled for its own checks (never for a box, and never a
          # dependency, which stays the shared build). Its test build is
          # linted as it is compiled, and its plain build once more for what
          # only exists outside tests - cargo clippy --all-targets, a crate
          # at a time, cached like the tests
          clippyRustc = pkgs.writeShellScriptBin "rustc" ''exec ${pkgs.clippy}/bin/clippy-driver "$@"'';
          linted =
            o:
            let
              u = unoptimised o;
            in
            u
            // {
              rust = clippyRustc;
              capLints = "forbid";
              extraRustcOpts = u.extraRustcOpts ++ [
                "-D"
                "warnings"
              ];
            };
          crateLint = n: [
            ((testGraph n).${n}.override (o: linted o // { buildTests = true; }))
            ((testGraph n).${n}.override linted)
          ];
          # The gate's tests (and dd's, which run a gate) read the pages from
          # the tree; the library's are checked against rclone itself; dd's
          # run git and ssh
          crateTest =
            n:
            let
              g = testGraph n;
            in
            (cargoNix.internal.crateWithTest {
              crate = g.${n};
              testCrate = g.${n}.override linted;
              testCrateFlags = [ ];
              testInputs = [
                pkgs.gitMinimal
                pkgs.openssh
                pkgs.rclone
              ];
              # the pages only where they are read: a page edit would run
              # every crate's tests again otherwise
              testPreRun =
                "export HOME=$TMPDIR"
                + pkgs.lib.optionalString (builtins.elem n [
                  "verify"
                  "dd"
                ]) " VERIFY_PAGES_DIR=${pages}";
              testPostRun = "";
            }).test;
          # every crate but the app, whose tests are its own check (tests-app)
          testedCrates = pkgs.lib.filter (n: n != "commonty") (builtins.attrNames cargoNix.workspaceMembers);
          # what crane builds from: dd for the app, the wasm and the app;
          # the box programs build per crate from Cargo.nix above
          sources = {
            dd = crateSrc "dd" [
              "client/cli"
              "client/gitremote"
            ];
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
          # the same, without the emulator and its system images: what a
          # build needs, and all a box fetches to build the apk
          androidBuildSdk =
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
              cmdLineToolsVersion = "11.0";
            }).androidsdk;
          androidHome = "${androidBuildSdk}/libexec/android-sdk";
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
            doCheck = false;
          };
          # the app: its window, and its network engine (app/build.rs links
          # it from here)
          appCommon = common // {
            buildInputs = appLibs;
            COMMONTY_NET_LIB_DIR = "${appNet}/lib";
          };
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
                    dummySrc = depsIn source;
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
              dummySrc = depsIn sources.app;
              version = "0.1.0";
              cargoExtraArgs = "-p commonty";
            }
          );
          webDist = pkgs.runCommand "dd-web-dist" { nativeBuildInputs = [ pkgs.wasm-bindgen-cli ]; } ''
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
            # a run's graph on the Git pages: where each job goes and where
            # each line bends (Eclipse's layout engine; loaded on that page alone)
            cp ${
              pkgs.fetchurl {
                url = "https://cdn.jsdelivr.net/npm/elkjs@0.12.0/lib/elk.bundled.js";
                hash = "sha256-EiLkT5U853Rq8jgB5yNwj45vQ2uLN3pqX8dVLzSjB7M=";
              }
            } $out/elk.js
          '';
          # The control plane (servers/system/control, docs/dev/control-plane.md): every box
          # runs dd-control; the witness is the same validator as wasm.
          control =
            let
              # each box's address on the relay and its TPM-held key; the
              # voters and rules from fleet/control.json
              boxList = builtins.fromJSON (builtins.readFile ./fleet/boxes.json);
              names = builtins.sort builtins.lessThan (builtins.attrNames boxList);
              withKeys = builtins.filter (n: boxList.${n} ? unlockKey) names;
              ctl = builtins.fromJSON (builtins.readFile ./fleet/control.json);
              config = {
                boxes = builtins.listToAttrs (
                  nixpkgs.lib.imap1 (i: n: {
                    name = n;
                    value = {
                      id = 100 + i;
                      house = boxList.${n}.siteId;
                      key = boxList.${n}.unlockKey;
                    };
                  }) withKeys
                );
                inherit (ctl) validators rules;
              };
              src = pkgs.lib.fileset.toSource {
                root = ./servers/system/control;
                fileset = pkgs.lib.fileset.unions [
                  ./servers/system/control/go.mod
                  ./servers/system/control/go.sum
                  ./servers/system/control/state
                  ./servers/system/control/validator
                  ./servers/system/control/wire
                  ./servers/system/control/cmd
                ];
              };
              dd-control = pkgs.buildGoModule {
                pname = "dd-control";
                version = "0.1.0";
                inherit src;
                vendorHash = "sha256-sYPGMtJetj/Ou1Q52s2tfRJkNcaiPlhuqIiq/H/1UjY=";
                subPackages = [ "cmd/dd-control" ];
              };
            in
            {
              configFile = pkgs.writeText "control-config.json" (builtins.toJSON config);
              inherit dd-control;
              # wasm for the relay's Durable Object, from the same vendored
              # modules (buildGoModule lists packages for the build machine,
              # which leaves out a js-only one)
              witness =
                pkgs.runCommand "dd-control-witness"
                  {
                    nativeBuildInputs = [ pkgs.go ];
                  }
                  ''
                    export HOME=$TMPDIR GOCACHE=$TMPDIR/cache GOFLAGS=-mod=vendor GOOS=js GOARCH=wasm CGO_ENABLED=0
                    cp -r ${src} src && chmod -R u+w src && cd src
                    cp -r ${dd-control.goModules} vendor
                    mkdir -p $out
                    go build -trimpath -ldflags="-s -w" -o $out/witness.wasm ./cmd/witness
                  '';
            };
          # What the app carries for its window, never fetched from a box:
          # the wasm bundle and the players (app/src/site.rs), and Photos'
          # and Passwords' clients (app/src/local.rs): the very builds the
          # boxes serve, so the code that holds a person's keys comes with
          # the app's signed release, Android's too. Source maps stay out:
          # they are for a debugger, and half the size.
          appWeb =
            let
              appClients =
                let
                  boxes = builtins.fromJSON (builtins.readFile ./fleet/boxes.json);
                  withRole =
                    r:
                    self.nixosConfigurations.${
                      nixpkgs.lib.findFirst (n: builtins.elem r boxes.${n}.roles) (throw "no ${r} box") (
                        builtins.attrNames boxes
                      )
                    }.config;
                  photos = withRole "photos";
                  vault = withRole "vault";
                in
                {
                  photos = photos.services.nginx.virtualHosts."photos.${photos.dd.domain}".locations."/".root;
                  vault = "${vault.services.vaultwarden.webVaultPackage}/share/vaultwarden/vault";
                };
            in
            pkgs.runCommand "commonty-app-web" { } ''
              mkdir -p $out/_dd/web
              cp ${webDist}/* $out/_dd/web/
              for c in photos:${appClients.photos} vault:${appClients.vault}; do
                dst=$out/_clients/''${c%%:*}
                (cd "''${c#*:}" && find -L . -type f ! -name '*.map' -exec install -Dm0644 {} "$dst/{}" \;)
              done
            '';
        in
        {
          # a package must be a derivation (flake check), not a source path
          pages = pkgs.runCommandLocal "dd-pages" { } "cp -r ${pages} $out";
          # what ci's style and lint jobs run (.forgejo/checks), pinned with
          # everything else
          ci-tools = pkgs.buildEnv {
            name = "ci-tools";
            paths = with pkgs; [
              python3
              gitMinimal
              yq-go
              nixfmt
              ruff
              shellcheck
              actionlint
              biome
              gitleaks
              typos
              cargo-audit
              nix-eval-jobs
            ];
          };
          # the cli, and `git remote add origin dd::...`, which dd repo calls too
          # `dd media` mounts libraries with rclone; the binary knows where it is
          dd = pkgs.symlinkJoin {
            name = "dd";
            paths = [
              (perCrate "dd")
              (perCrate "git-remote-dd")
            ];
            nativeBuildInputs = [ pkgs.makeWrapper ];
            postBuild = "wrapProgram $out/bin/dd --set DD_RCLONE ${pkgs.rclone}/bin/rclone";
          };
          # The release agent, on every box. Server side like verify: the
          # release crate holds the file format the cli signs and this
          # binary checks, and nothing that needs a keyring.
          agent = perCrate "release";
          # The verifier behind nginx's auth_request. Built separately from dd
          # rather than as another binary in the same derivation: this one
          # runs on a server and has no business pulling in the keyring/dbus
          # stack that the cli needs.
          verify = perCrate "verify";
          # the manager behind the Games tile (nix/modules/games/games.nix)
          games = perCrate "games";
          # one file, one viewer, in memory: the compute side of the
          # encrypted libraries (modules/library/transcode.nix)
          transcode = perCrate "transcode";
          # The app: the same crates as dd behind a window (app/). Wrapped
          # so the webview finds its schemas and gio modules; the dmabuf
          # renderer is off because on nvidia it draws a blank window.
          app = (crateWith appCommon "commonty" sources.app "-p commonty").overrideAttrs (old: {
            nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.wrapGAppsHook3 ];
            preBuild = (old.preBuild or "") + ''
              cp -r ${appWeb}/. app/web/ && chmod -R u+w app/web
            '';
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
                # On NVIDIA's own driver off NixOS the webview found no GL it
                # could load and drew in software: scrolling Photos lagged
                # behind the browser. nixglhost hands it the machine's driver.
                # Anywhere else it starts as it always did.
                mv $out/bin/commonty $out/bin/.commonty-gpu
                cat > $out/bin/commonty <<EOF
                #!${pkgs.runtimeShell}
                if [ -e /proc/driver/nvidia/version ] && [ ! -e /etc/NIXOS ]; then
                  exec ${nix-gl-host.packages.${system}.default}/bin/nixglhost $out/bin/.commonty-gpu "\$@"
                fi
                exec $out/bin/.commonty-gpu "\$@"
                EOF
                chmod +x $out/bin/commonty
              '';
          });
          # What a stranger downloads, for one release's version: every
          # file built the same way on each box that builds it, so the boxes
          # can compare and the laptop signs what they agree on
          # (docs/dev/app-release.md). `dd release app` asks for these by version.
          appRelease =
            version:
            let
              cargoVendor = craneLib.vendorCargoDeps { cargoLock = ./Cargo.lock; };
              toolchain =
                targets:
                # the Rust the rest of the flake compiles with, plus the targets
                # nixpkgs' rustc has no std for
                (pkgs.extend rust-overlay.overlays.default).rust-bin.stable.${pkgs.rustc.version}.minimal.override {
                  inherit targets;
                };
              linux = import ./nix/app-release/linux.nix {
                inherit
                  pkgs
                  version
                  appWeb
                  cargoVendor
                  ;
                src = sources.app;
                netSrc = ./app/net;
                netVendor = appNet.goModules;
              };
              windows = import ./nix/app-release/windows.nix {
                inherit
                  pkgs
                  version
                  appWeb
                  cargoVendor
                  ;
                rustToolchain = toolchain [ "x86_64-pc-windows-msvc" ];
                appSrc = sources.app;
                ddSrc = sources.dd;
                netSrc = ./app/net;
                netVendor = appNet.goModules;
              };
              # What each build needs of the box that runs it: build orders
              # go only to boxes that have it (client/cli/src/build_order.rs).
              # The Linux build boots a VM of 6 GB.
              need = n: d: d // { needs = n; };
            in
            {
              linux = need {
                kvm = true;
                ramGB = 12;
              } linux;
              windows = need { ramGB = 8; } windows.installer;
              dd-windows = need { ramGB = 6; } windows.dd;
              android = need { ramGB = 8; } (
                import ./nix/app-release/android.nix {
                  inherit
                    pkgs
                    version
                    cargoVendor
                    ;
                  inherit appWeb;
                  androidSdk = androidBuildSdk;
                  rustToolchain = toolchain [
                    "aarch64-linux-android"
                    "x86_64-linux-android"
                  ];
                  src = sources.app;
                  netAndroid = appNetAndroid;
                }
              );
              # the proof each starts where a stranger would run it
              tests.linux = import ./nix/app-release/test-linux.nix { inherit pkgs linux; };
              tests.windows = import ./nix/app-release/test-windows.nix {
                inherit pkgs;
                installer = windows.installer;
                dd = windows.dd;
              };
            };
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
          # is a passkey (client/web). The verifier serves this directory.
          # A film is HLS, because that is what the box's transcode makes
          # and what lets a viewer seek. Safari plays a playlist by itself;
          # nothing else does, so the page needs a player. Pinned by hash
          # and served from this directory beside our own wasm: no
          # third-party blob in the tree, and the page fetches it from the
          # box like everything else it loads.
          web = webDist;
          # The relay Worker: what `dd release publish` uploads as
          # control.<domain>, with this release's config inside
          control-worker = pkgs.runCommand "dd-control-worker" { } ''
            mkdir -p $out
            cp ${./servers/system/control/worker/worker.js} $out/worker.js
            cp ${pkgs.go}/share/go/lib/wasm/wasm_exec.js $out/wasm_exec.js
            cp ${control.witness}/witness.wasm $out/witness.wasm
            echo "export default $(cat ${control.configFile});" > $out/config.js
          '';
          dd-control = control.dd-control;
          control-config = control.configFile;
          # every service's needs, typed (nix/lib/needs.nix): what placement
          # will read, with fleet/roles.json. Nothing places roles yet;
          # fleet/boxes.json is written by hand and checked by role-fit
          needs = pkgs.writeText "needs.json" (
            builtins.toJSON (import ./nix/lib/needs.nix { lib = nixpkgs.lib; })
          );
          # The id Worker (servers/system/id): sign-in for every box, at
          # id.<domain>. Static: the page, its script, the site's styles
          id-worker = pkgs.runCommand "dd-id-worker" { } ''
            mkdir -p $out
            cp ${./servers/system/id}/* $out/
            cp ${./servers/web-shared/home.css} $out/home.css
            cp ${./servers/web-shared/bar.css} $out/bar.css
            cp ${./servers/web-shared/favicon.svg} $out/favicon.svg
            cp ${./servers/web-shared/fonts}/*.woff2 $out/
          '';
          # The DNS Worker (servers/system/dns): what `dd release publish`
          # uploads as dns.<domain>. Each box's TPM-held key, and the names
          # its own nginx serves over https - all it may have a challenge for;
          # and the fleet's names as the gateway's config has them
          dns-worker =
            let
              boxList = builtins.fromJSON (builtins.readFile ./fleet/boxes.json);
              withKeys = nixpkgs.lib.filterAttrs (_: b: b ? unlockKey) boxList;
              gateways = builtins.filter (n: self.nixosConfigurations.${n}.config.dd ? public) (
                builtins.attrNames boxList
              );
              fleet = {
                RECORDS =
                  assert nixpkgs.lib.assertMsg (
                    builtins.length gateways <= 1
                  ) "the DNS Worker keeps one gateway's names; these are all gateways: ${toString gateways}";
                  if gateways == [ ] then
                    null
                  else
                    import ./nix/lib/fleet-names.nix self.nixosConfigurations.${builtins.head gateways}.config
                    // {
                      # every box's address on the fleet's network: an A at one
                      # of these is a record of ours
                      fleet = map (n: self.nixosConfigurations.${n}.config.dd.box.net) (builtins.attrNames boxList);
                      # a box with names of its own (metrics.<box>.<domain>):
                      # *.<box>.<domain> at its address on the fleet's network
                      boxes = nixpkgs.lib.mapAttrs (n: _: self.nixosConfigurations.${n}.config.dd.box.net) (
                        nixpkgs.lib.filterAttrs (
                          n: _: self.nixosConfigurations.${n}.config.dd.boxMetrics.host != ""
                        ) boxList
                      );
                    };
                BOXES = nixpkgs.lib.mapAttrs (_: b: b.unlockKey) withKeys;
                NAMES = nixpkgs.lib.mapAttrs (
                  n: _: import ./nix/lib/https-names.nix nixpkgs.lib self.nixosConfigurations.${n}.config
                ) withKeys;
              };
            in
            pkgs.runCommand "dd-dns-worker" { } ''
              mkdir -p $out
              cp ${./servers/system/dns/worker.js} $out/worker.js
              cp ${pkgs.writeText "fleet.js" ''
                export const BOXES = ${builtins.toJSON fleet.BOXES};
                export const NAMES = ${builtins.toJSON fleet.NAMES};
                export const RECORDS = ${builtins.toJSON fleet.RECORDS};
              ''} $out/fleet.js
            '';
          # The mail Worker (client/mail): what `dd release publish` uploads
          # to Cloudflare, file for file, and what the boxes compare the
          # deployed Worker with. The checks are the boxes' own Rust in wasm;
          # the member list and release key are this release's.
          mail-worker =
            let
              mailCommon = wasmCommon // {
                src = crateSrc "mail" [ "client/mail" ];
                cargoExtraArgs = "-p dd-mail";
              };
              mailWasm = craneWasm.buildPackage (
                mailCommon
                // {
                  pname = "dd-mail";
                  version = "0.1.0";
                  cargoArtifacts = craneWasm.buildDepsOnly (
                    mailCommon
                    // {
                      pname = "dd-mail-deps";
                      dummySrc = depsSrc;
                      version = "0.1.0";
                    }
                  );
                }
              );
              # fleet/policy.json mailAliases: <alias>@<domain> to a member.
              # An alias is lowercase a-z, 0-9 and -, never a person's name
              policy = nixpkgs.lib.importJSON ./fleet/policy.json;
              aliases = policy.mailAliases or { };
              people = [ policy.owner ] ++ builtins.attrNames (policy.people or { });
              badAlias =
                a: m:
                builtins.match "[a-z0-9-]{1,64}" a == null
                || builtins.match "[a-z0-9][a-z0-9._-]{0,63}" m == null
                || builtins.elem a people
                || a == m;
              bad = builtins.attrNames (nixpkgs.lib.filterAttrs badAlias aliases);
            in
            assert nixpkgs.lib.assertMsg (
              bad == [ ]
            ) "fleet/policy.json mailAliases: not an alias here: ${builtins.concatStringsSep ", " bad}";
            pkgs.runCommand "dd-mail-worker"
              {
                nativeBuildInputs = [
                  pkgs.wasm-bindgen-cli
                  pkgs.jq
                ];
              }
              ''
                mkdir -p $out
                wasm-bindgen --target web --no-typescript --out-dir $out ${mailWasm}/lib/dd_mail.wasm
                cp ${./client/mail/worker}/* $out/
                cp ${./servers/web-shared/home.css} $out/home.css
                cp ${./servers/web-shared/bar.css} $out/bar.css
                cp ${./servers/web-shared/fonts}/*.woff2 $out/
                {
                  echo "export const MEMBERS = $(jq -c . ${./fleet/members.json} | jq -R .);"
                  echo "export const RELEASE = $(tr -d '[:space:]' < ${./fleet/release.pub} | jq -R .);"
                  echo "export const ALIASES = $(jq -c '.mailAliases // {}' ${./fleet/policy.json});"
                  # each box's TPM-held unlock key, where it has one (unlock.js)
                  echo "export const BOXES = $(jq -c 'with_entries(select(.value.unlockKey) | .value = .value.unlockKey)' ${./fleet/boxes.json});"
                } > $out/fleet.js
              '';

          # the checks, on the same compiled artifacts as the binaries: fmt
          # and clippy in seconds, the tests once, all cached by content and
          # shared between the runner boxes through the bucket. ci builds
          # these instead of running cargo a second and third time.
          # Cargo.nix follows Cargo.lock: the box programs build from it, and
          # a stale one would build the old versions without a word
          cargo-nix =
            let
              lib = pkgs.lib;
              lock = builtins.fromTOML (builtins.readFile ./Cargo.lock);
              want = lib.sort lib.lessThan (map (p: "${p.name} ${p.version}") lock.package);
              have = lib.sort lib.lessThan (
                lib.mapAttrsToList (_: c: "${c.crateName} ${c.version}") cargoNix.internal.crates
              );
            in
            if want == have then
              pkgs.runCommandLocal "cargo-nix-follows-cargo-lock" { } "touch $out"
            else
              throw "Cargo.nix does not match Cargo.lock (only in Cargo.lock: ${toString (lib.subtractLists have want)}; only in Cargo.nix: ${toString (lib.subtractLists want have)}). Run: nix run nixpkgs#crate2nix -- generate";
          fmt = craneLib.cargoFmt {
            inherit src;
            pname = "dd";
            version = "0.1.0";
          };
          # every crate's lint, each its own build (crateLint above)
          clippy = pkgs.linkFarm "clippy" (
            pkgs.lib.concatMap (
              n:
              pkgs.lib.imap0 (i: path: {
                name = "${n}${pkgs.lib.optionalString (i == 0) "-tests"}";
                inherit path;
              }) (crateLint n)
            ) testedCrates
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
              doInstallCargoArtifacts = false;
            }
          );
          # the app's own tests, on the app's cached dependency build: the
          # workspace's tests leave the app out (testedCrates)
          tests-app = craneLib.cargoTest (
            appCommon
            // {
              src = sources.app;
              cargoArtifacts = appArtifacts;
              pname = "commonty";
              version = "0.1.0";
              cargoExtraArgs = "-p commonty";
              doCheck = true;
              doInstallCargoArtifacts = false;
            }
          );
          # the app's network engine: go vet, and its tests (app/net/*_test.go)
          net-check = appNet.overrideAttrs (_: {
            pname = "commonty-net-check";
            doCheck = true;
            checkPhase = ''
              runHook preCheck
              go vet ./...
              go test ./...
              runHook postCheck
            '';
          });
          # every crate's tests, each its own build: one link to them all,
          # so CI asks for one check and gets every crate's answer, cached
          # crate by crate
          tests = pkgs.linkFarm "tests" (
            map (n: {
              name = n;
              path = crateTest n;
            }) testedCrates
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
            pkgs.fuse3 # fusermount3, which dd media runs to unmount rclone's mount
            pkgs.sops # `dd secret run -- ...` runs this
            pkgs.age
            pkgs.age-plugin-tpm # sops encrypts to the boxes' TPM keys through it
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
          "tests-app"
          "net-check"
          "android-sdk"
          "appRelease"
        ]
        // {
          # `dd secret run` falls back to this when sops is not on PATH. The
          # fleet's age key goes into that process's environment, so it is
          # this repo's pinned nixpkgs and not whatever unstable is serving
          # at the moment of use.
          # (with the plugin that encrypts to the boxes' TPM keys)
          sops = pkgs.writeShellApplication {
            name = "sops";
            runtimeInputs = [ pkgs.age-plugin-tpm ];
            text = ''exec ${pkgs.sops}/bin/sops "$@"'';
          };
        };

      # the downloads for a version: `(appRelease "1.2.3").linux`
      legacyPackages.${system} = {
        inherit (rust) appRelease;
        # What flake_check evaluates, one attribute at a time in its own
        # worker (nix-eval-jobs): every check, package and dev shell, and
        # every box's system. One evaluator for all of it grew to 10.5G and
        # took node2 out of memory beside a VM test.
        ciEval = {
          checks = self.checks.${system};
          packages = self.packages.${system};
          devShells = self.devShells.${system};
          systems = nixpkgs.lib.mapAttrs (_: c: c.config.system.build.toplevel) self.nixosConfigurations;
        };
        # which runner boxes can take each of the runner's variants, from its
        # needs and the boxes' reports: CI places its jobs by this
        ciRunners = import ./nix/lib/ci-runners.nix { lib = nixpkgs.lib; };
        # The four downloads still evaluate: their recipes only, never a build
        # of them (that is the boxes', after a release). Not one of `checks`:
        # the Linux image's package list and the web bundle are made by a
        # build and read back (import from derivation), which `nix flake
        # check` (no builds) cannot do once a store has been cleaned. The
        # app's CI job builds this instead.
        appReleaseEvaluates = pkgs.writeText "app-release-evaluates" (
          toString (
            map (d: builtins.unsafeDiscardOutputDependency d.drvPath) (
              with rust.appRelease "0.0.0";
              [
                linux
                windows
                dd-windows
                android
              ]
            )
          )
        );
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
            cargo-nix
            fmt
            clippy
            clippy-app
            tests-app
            net-check
            tests
            ;
          plaintext-label = import ./nix/tests/plaintext-label.nix args;
          ci = import ./nix/tests/ci.nix (args // { inherit vmTests; });
          boxes = import ./nix/tests/boxes.nix args;
          box-reports = import ./nix/tests/box-reports.nix args;
          role-fit = import ./nix/tests/role-fit.nix args;
          # anything deleted is gone from every box and backup within 90 days
          retention = import ./nix/tests/retention.nix args;
          # every unit our modules define is in some service's needs, and no
          # timer of ours counts from boot
          needs-units = import ./nix/tests/needs-units.nix args;
          # every box can carry what its roles bring, by the needs; and the
          # roles say truly what they bring (nix/lib/place.nix)
          placement = import ./nix/tests/placement.nix args;
          nginx = import ./nix/tests/nginx.nix args;
          # no member's figures in the fleet's metrics: usage is heartbeat
          # lines, never a Prometheus sample (storage/usage.nix)
          usage-facts = pkgs.runCommand "usage-facts-are-not-metrics" { } ''
            printf 'tom@x.org\t2048\nbad name\t1\nsarah\t7\n' > rows
            ${pkgs.gawk}/bin/awk -F '\t' -v svc=code -v suffix=@x.org \
              -f ${./nix/modules/storage/usage-facts.awk} rows > got
            printf 'usage/code/tom 2048\nusage/code/sarah 7\n' | cmp - got
            if ${pkgs.gnugrep}/bin/grep -rn 'dd_usage' ${./nix/modules}; then exit 1; fi
            # node-exporter serves /var/lib/dd-facts/*.prom: nothing writes usage there but to remove it
            if ${pkgs.gnugrep}/bin/grep -rn 'usage-[^ ]*\.prom' ${./nix/modules} | ${pkgs.gnugrep}/bin/grep -v 'rm -f'; then exit 1; fi
            touch $out
          '';
          # the clients' default names are the fleet's (client/identity/src/fleet.rs)
          fleet-defaults =
            let
              policy = nixpkgs.lib.importJSON ./fleet/policy.json;
              boxes = nixpkgs.lib.importJSON ./fleet/boxes.json;
              src = ./client/identity/src/fleet.rs;
            in
            pkgs.runCommand "fleet-defaults" { } ''
              grep -qF '"${policy.domain}"' ${src}
              grep -qF '"${policy.repo}"' ${src}
              grep -qF '"${boxes.node1.net}"' ${src}
              touch $out
            '';
          # the Terms of use a sign-up agrees to (identity::TERMS, kept in
          # each new entry) are the ones the page says last changed then
          terms-version =
            let
              flat = f: builtins.replaceStrings [ "\n" ] [ " " ] (builtins.readFile f);
              m = builtins.match ''.*pub const TERMS: &str = "([0-9-]+)";.*'' (flat ./client/identity/src/lib.rs);
              said = "These terms last changed on ${builtins.head m}.";
            in
            pkgs.runCommand "terms-version" { } (
              if m != null && nixpkgs.lib.hasInfix said (flat ./docs/policies/terms.md) then
                "touch $out"
              else
                "echo 'identity::TERMS and docs/policies/terms.md (\"These terms last changed on ...\") disagree' >&2; exit 1"
            );
          # the mail Worker in a browser, against a fake Cloudflare
          mail-worker = import ./nix/tests/mail-worker.nix args;
          safe-rd = import ./nix/tests/safe-rd.nix args;
          sso = import ./nix/tests/sso.nix args;
          dns-worker = import ./nix/tests/dns-worker.nix args;
          # the control plane: relay, witness and three validators, offline
          control = import ./nix/tests/control.nix args;
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
      # the per-box wiring (who its peers are, the boxes Settings shows)
      # is derived from the same list.
      nixosConfigurations =
        let
          boxes = builtins.fromJSON (builtins.readFile ./fleet/boxes.json);
          lib = nixpkgs.lib;
          directoryOf =
            name: box:
            if box.public then
              "https://files.${(lib.importJSON ./fleet/policy.json).domain}/_dd/directory"
            else
              "http://${box.net}:4181/_dd/directory";
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
              publicAddr = "${box.net}:3901";
              peers = lib.mapAttrsToList (_: b: "${b.garage.id}@${b.net}:3901") (
                lib.filterAttrs (n: b: n != name && b.garage ? id) storage
              );
            };
          };
          # The storage pool, for a box: its own garage, or a storage box's
          # over the fleet's network - one in the same house when there is
          # one, then any - each serves the whole cluster. Closures (the
          # nix-cache bucket), backups, the vault's files and the gateway's
          # s3. all go there (dd.box.s3).
          poolFor =
            box:
            if builtins.elem "storage" box.roles then
              "127.0.0.1"
            else
              let
                order = lib.sort (a: b: a.name < b.name) (
                  lib.mapAttrsToList (name: b: b // { inherit name; }) storage
                );
                near = lib.filter (b: b.siteId == box.siteId) order;
              in
              (builtins.head (if near != [ ] then near else order)).net;
          cacheOf =
            name: box:
            let
              host = poolFor box;
            in
            {
              dd.agent.cache = "s3://nix-cache?endpoint=${host}:3900&scheme=http&region=us-east-1";
              dd.box.s3 = "http://${host}:3900";
            };
          # what a member's device may reach for games, on any box: each
          # games box's own list (dd.games.reach: its range and every port
          # a game insists on)
          fleetGamePorts = lib.unique (
            lib.concatMap (n: self.nixosConfigurations.${n}.config.dd.games.reach) (
              builtins.attrNames (lib.filterAttrs (_: b: builtins.elem "game" b.roles) boxes)
            )
          );
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
                (import ./nix/modules/box/host.nix ./fleet/boxes/${name})
                {
                  networking.hostName = name;
                  dd.box.net = box.net;
                  dd.box.site = box.siteId;
                  dd.box.region = box.regionId;
                  # the same house's boxes alone: its Wi-Fi and its page
                  dd.verify.housePeers = lib.mapAttrsToList directoryOf (
                    lib.filterAttrs (n: b: n != name && b.siteId == box.siteId) boxes
                  );
                  # itself included: Settings' Network tab is the whole fleet
                  dd.verify.fleet = lib.mapAttrs (_: b: b.net) boxes;
                  # whose each box is, and where its own metrics answer
                  dd.verify.boxes = lib.mapAttrs (n: _: {
                    owner = (lib.importJSON ./fleet/boxes/${n}/box.json).owner or null;
                    metrics = self.nixosConfigurations.${n}.config.dd.boxMetrics.host;
                  }) boxes;
                  # the other boxes, as the agent checks a release did not
                  # cost this box its way off itself
                  # a port each other box listens on, or refuses from: the gate's
                  # (in the house it answers; out of it the firewall is silent)
                  # and the front door's. Either answering is reach. An agent
                  # from before refusals counted needs the first: that is what
                  # took release 211 back on node1, where node2 has no 443.
                  dd.agent.reach = lib.concatLists (
                    lib.mapAttrsToList (_: b: [
                      "${b.net}:4181"
                      "${b.net}:443"
                    ]) (lib.filterAttrs (n: _: n != name) boxes)
                  );
                }
                (lib.optionalAttrs (builtins.elem "gateway" box.roles) {
                  dd.headscale.gamePorts = fleetGamePorts;
                  dd.headscale.owners = lib.mapAttrs' (
                    n: b: lib.nameValuePair b.net (lib.importJSON ./fleet/boxes/${n}/box.json).owner
                  ) (lib.filterAttrs (n: b: b ? net && builtins.pathExists ./fleet/boxes/${n}/box.json) boxes);
                  # every box may ask for its own key, signed and sealed by its TPM
                  dd.headscale.boxes =
                    lib.mapAttrs
                      (n: b: {
                        key = b.unlockKey;
                        recipient = (lib.importJSON ./fleet/boxes/${n}/hardware.json).secretsKey;
                      })
                      (
                        lib.filterAttrs (n: b: b ? unlockKey && builtins.pathExists ./fleet/boxes/${n}/hardware.json) boxes
                      );
                })
              ]
              ++ map (r: ./nix/roles/${r}.nix) box.roles
              ++ lib.optional (builtins.elem "storage" box.roles) (garageOf name box)
              ++ [ (cacheOf name box) ]
              ++ [
                lanzaboote.nixosModules.lanzaboote
                ./nix/modules/box/secure-boot.nix
                ./nix/modules/box/disk-unlock.nix
                ./nix/modules/box/control.nix
                ./nix/modules/box/ports.nix
                ./nix/modules/box/report.nix
                {
                  dd.control.enable = true;
                  dd.control.roles = box.roles;
                }
              ]
              ++ lib.optional (needsSops box.roles) sops-nix.nixosModules.sops
              ++ [ disko.nixosModules.disko ]
              ++ lib.optional (builtins.elem "observe" box.roles) {
                # this box's own sidecar; the others' history is read from the
                # bucket, never from them (README: no box talks to another)
                dd.thanos.sidecars = [ "127.0.0.1:10901" ];
              };
            };
        in
        lib.mapAttrs mkBox boxes;
    };
}
