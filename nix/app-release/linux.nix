# The Linux downloads: the .deb and the AppImage, built inside a stock
# Ubuntu 22.04 (packages from a dated snapshot) so they run on any desktop
# from that year on. Built in the flake's own sandbox they would ask for the
# glibc nix has, which no Ubuntu or Fedora carries. The compiler, cargo,
# tauri and Go are the flake's own; what they link against is Ubuntu's.
{
  pkgs,
  version,
  src,
  cargoVendor,
  netSrc,
  netVendor,
  webDist,
}:
let
  image = pkgs.vmTools.diskImageFuns.ubuntu2204x86_64 {
    # room for the build itself, which happens on this disk
    size = 32768;
    extraPackages = [
      "build-essential"
      "pkg-config"
      "file"
      "patchelf"
      "xdg-utils"
      "libwebkit2gtk-4.1-dev"
      "libgtk-3-dev"
      "librsvg2-dev"
      "libssl-dev"
      # what the AppImage carries for the players (bundleMediaFramework);
      # bad is what streams (MSE), libav decodes H.264 and AAC
      "gstreamer1.0-plugins-base"
      "gstreamer1.0-plugins-good"
      "gstreamer1.0-plugins-bad"
      "gstreamer1.0-libav"
    ];
  };
  # what tauri's AppImage step fetches itself on a desktop, pinned here:
  # each by an address that does not move and checked by hash
  tool = url: hash: pkgs.fetchurl { inherit url hash; };
  tools = {
    "AppRun-x86_64" =
      tool "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64" "sha256-8wFApDoKWeRtshve/fdJuenyxpRukq+rus+YuK5z+08=";
    "linuxdeploy-x86_64.AppImage" =
      tool "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy-07333c6/linuxdeploy-x86_64.AppImage" "sha256-NqLX4nTRLhBQ0Ons/hHTOe1Ucgsr7EZMKG1T+LB/XGI=";
    "linuxdeploy-plugin-gtk.sh" =
      tool "https://raw.githubusercontent.com/tauri-apps/linuxdeploy-plugin-gtk/dda522bce37387f1b853d9095713bfaa924c8423/linuxdeploy-plugin-gtk.sh" "sha256-eATJ7vE+Wb8ng6rZiC75248/P56NYxh0sdNI1VCjaT8=";
    "linuxdeploy-plugin-gstreamer.sh" =
      tool "https://raw.githubusercontent.com/tauri-apps/linuxdeploy-plugin-gstreamer/2a2e67491c32995a3f279ad0ecbe77abd512b42a/linuxdeploy-plugin-gstreamer.sh" "sha256-wQe0nYTtv/xqsibtEAfgYmpPeqLDo2t3gr72I1HUnpQ=";
    "linuxdeploy-plugin-appimage.AppImage" =
      tool "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/1-alpha-20250213-1/linuxdeploy-plugin-appimage-x86_64.AppImage" "sha256-mS1QKiSOFKsYVEjd9vbn0lVYy4TUYjw1TDrzUMJfzLM=";
  };
  # what the AppImage starts with, before it mounts itself
  runtime = pkgs.fetchurl {
    url = "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64";
    hash = "sha256-L8qLRDySUQ8Ug6iD9gBhrQm0a5eLJjHIB82HOkfsJg0=";
  };
  nixTools = pkgs.lib.makeBinPath [
    pkgs.rustc
    pkgs.cargo
    pkgs.cargo-tauri
    pkgs.go
  ];
in
pkgs.vmTools.runInLinuxImage (
  pkgs.stdenv.mkDerivation {
    pname = "commonty-linux";
    inherit version;
    diskImage = image;
    diskImageFormat = "qcow2";
    # node2 has 14 GB in all; the build fits in six
    memSize = 6144;
    enableParallelBuilding = true;
    # one fixed time for every file the packages hold, so two boxes make
    # the same bytes
    SOURCE_DATE_EPOCH = "315532800";
    buildCommand = ''
      set -euo pipefail
      export PATH=${nixTools}:/usr/bin:/bin:/usr/sbin:/sbin
      # on the image's disk: /tmp in the VM is memory, and the build and
      # the AppImage's tree outgrow it
      export HOME=/work/home XDG_CACHE_HOME=/work/cache TMPDIR=/work/tmp
      mkdir -p $HOME $XDG_CACHE_HOME/tauri $TMPDIR /work/netlib
      ${pkgs.lib.concatStrings (
        pkgs.lib.mapAttrsToList (n: f: ''
          install -m755 ${f} $XDG_CACHE_HOME/tauri/${n}
        '') tools
      )}

      # the network engine, with Ubuntu's C compiler
      cp -r ${netSrc} /work/net && chmod -R u+w /work/net
      cp -r ${netVendor} /work/net/vendor
      (cd /work/net && CGO_ENABLED=1 CC=gcc GOFLAGS="-mod=vendor -trimpath" \
        GOTOOLCHAIN=local GOPROXY=off GOCACHE=/work/gocache \
        go build -buildmode=c-archive -o /work/netlib/libcommontynet.a .)

      cp -r ${src} /work/src && chmod -R u+w /work/src
      cd /work/src
      mkdir -p app/web/_dd/web
      cp ${webDist}/* app/web/_dd/web/
      mkdir -p .cargo
      cp ${cargoVendor}/config.toml .cargo/config.toml

      export CARGO_NET_OFFLINE=true
      export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc
      export COMMONTY_NET_LIB_DIR=/work/netlib COMMONTY_VERSION=${version}
      export APPIMAGE_EXTRACT_AND_RUN=1 NO_STRIP=1 LDAI_RUNTIME_FILE=${runtime}
      export CARGO_BUILD_JOBS=$NIX_BUILD_CORES
      cd app
      cargo tauri build --verbose --config '{"version":"${version}"}' --bundles deb,appimage -- --locked

      mkdir -p $out
      # tauri's .deb keeps each file's build time, in whatever order the
      # disk lists them; Ubuntu's dpkg-deb packs it again sorted, every
      # time clamped to SOURCE_DATE_EPOCH
      dpkg-deb -R /work/src/target/release/bundle/deb/*.deb /work/deb
      # and its checksum list in the disk's order too, which differs from
      # one machine to the next (v0.1.25: node1 and node2 disagreed on it)
      LC_ALL=C sort -k2 -o /work/deb/DEBIAN/md5sums /work/deb/DEBIAN/md5sums
      dpkg-deb --root-owner-group -Zxz --build /work/deb $out/commonty_${version}_amd64.deb
      cp /work/src/target/release/bundle/appimage/*.AppImage $out/commonty_${version}_amd64.AppImage
    '';
  }
)
