# The Windows downloads, cross-built here: the installer and dd.exe. Rust
# compiles for msvc with clang and lld (cargo-xwin) against Microsoft's
# C runtime and SDK, fetched once by version and checked by hash; the Go
# engine is a dll from mingw, because cgo cannot build for msvc and a dll
# is loaded by name, whoever built it; NSIS makes the installer.
{
  pkgs,
  version,
  rustToolchain,
  appSrc,
  ddSrc,
  cargoVendor,
  netSrc,
  netVendor,
  appWeb,
}:
let
  lib = pkgs.lib;
  llvm = pkgs.llvmPackages_21;
  # Microsoft's C runtime and Windows SDK, the parts a desktop x64 program
  # links with. Downloading them means accepting Microsoft's licence for
  # them, which is what a Windows build machine with Visual Studio does too.
  msvc = pkgs.stdenvNoCC.mkDerivation {
    name = "msvc-crt-14.44.17.14-sdk-10.0.26100";
    nativeBuildInputs = [ pkgs.xwin ];
    SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
    outputHash = "sha256-UFQjsFVBwcF/9e9tVFoG0Z1JySxyTnFqoaRwr/tUWzA=";
    buildCommand = ''
      xwin --accept-license --cache-dir $TMPDIR/cache --arch x86_64 --variant desktop \
        --manifest-version 17 --crt-version 14.44.17.14 --sdk-version 10.0.26100 \
        splat --output $TMPDIR/splat
      # splat moves files, which it cannot do across to the store's disk
      cp -r $TMPDIR/splat $out
    '';
  };
  # the engine as a dll, with the import library lld-link wants beside it
  net =
    pkgs.runCommand "commonty-net-windows"
      {
        nativeBuildInputs = [
          pkgs.go
          pkgs.pkgsCross.mingwW64.stdenv.cc
        ];
      }
      ''
        cp -r ${netSrc} net && chmod -R u+w net
        cp -r ${netVendor} net/vendor
        cd net
        export HOME=$TMPDIR GOCACHE=$TMPDIR/go GOFLAGS="-mod=vendor -trimpath" GOTOOLCHAIN=local GOPROXY=off
        mkdir -p $out
        CGO_ENABLED=1 GOOS=windows GOARCH=amd64 CC=x86_64-w64-mingw32-gcc \
          go build -buildmode=c-shared \
            -ldflags "-s -w -buildid= -extldflags '-static -Wl,--out-implib,$out/commontynet.lib'" \
            -o $out/commontynet.dll .
      '';
  nsisUtils = pkgs.fetchurl {
    url = "https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll";
    hash = "sha256-W6FDtdtKh9MtbngC4DMzCq5Wy86r4NHjukGUg4WtRwk=";
  };
  target = "x86_64-pc-windows-msvc";
  # one build, two uses: what every Windows build here sets up
  build =
    name: src: script:
    pkgs.stdenv.mkDerivation {
      inherit name src;
      nativeBuildInputs = [
        rustToolchain
        pkgs.cargo-xwin
        pkgs.cargo-tauri
        pkgs.nsis
        llvm.clang-unwrapped
        llvm.lld
        llvm.llvm
      ];
      SOURCE_DATE_EPOCH = "315532800";
      dontConfigure = true;
      dontFixup = true;
      buildPhase = ''
        # the pinned runtime and SDK above, for x64 only: cargo-xwin fetches
        # whatever arch it is not told it already has
        export HOME=$TMPDIR XDG_CACHE_HOME=$TMPDIR/cache XWIN_CACHE_DIR=$TMPDIR/xwin XWIN_ARCH=x86_64
        mkdir -p $XWIN_CACHE_DIR/xwin
        cp -r ${msvc}/. $XWIN_CACHE_DIR/xwin/ && chmod -R u+w $XWIN_CACHE_DIR
        echo x86_64 > $XWIN_CACHE_DIR/xwin/DONE
        install -Dm644 ${cargoVendor}/config.toml .cargo/config.toml
        # No debug info (none ships), and the header's stamp a hash of the
        # contents: lld-link otherwise stamps the time and names the debug
        # info after rustc's randomly named temporary files
        printf '\n[target.${target}]\nrustflags = ["-C", "link-arg=/Brepro", "-C", "link-arg=/DEBUG:NONE", "-C", "strip=symbols"]\n' >> .cargo/config.toml
        export CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=$NIX_BUILD_CORES
        export COMMONTY_NET_LIB_DIR=${net} COMMONTY_VERSION=${version}
        export COMMONTY_RELEASE_KEY=$(tr -d '[:space:]' < ${../../fleet/release.pub})
        export COMMONTY_REPO=${(pkgs.lib.importJSON ../../fleet/policy.json).repo}
        ${script}
      '';
      installPhase = "true";
    };
in
{
  inherit msvc net;
  installer = build "commonty-windows-${version}" appSrc ''
    install -Dm644 ${nsisUtils} $XDG_CACHE_HOME/tauri/NSIS/Plugins/x86-unicode/additional/nsis_tauri_utils.dll
    cp -r ${appWeb}/. app/web/ && chmod -R u+w app/web
    # the card writer the installer runs elevated (RunAs, no manifest: its
    # list needs no admin rights)
    cargo xwin build --release --locked --target ${target} -p hubwriter
    cd app
    # the engine goes beside the exe, where Windows looks for a dll first;
    # the hooks keep file times out of the installer
    cargo tauri build --verbose --runner cargo-xwin --target ${target} --bundles nsis \
      --config '{"version":"${version}","bundle":{"resources":{"${net}/commontynet.dll":"commontynet.dll","../target/${target}/release/dd-hub-writer.exe":"dd-hub-writer.exe"},"windows":{"nsis":{"installerHooks":"${./nsis-hooks.nsh}"}}}}' \
      -- --locked
    mkdir -p $out
    cp ../target/${target}/release/bundle/nsis/*.exe $out/commonty-setup.exe
  '';
  dd = build "dd-windows-${version}" ddSrc ''
    cargo xwin build --release --locked --target ${target} -p dd
    install -Dm755 target/${target}/release/dd.exe $out/dd.exe
  '';
}
