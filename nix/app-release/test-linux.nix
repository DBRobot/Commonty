# What a stranger's desktop does with the Linux downloads, on stock Ubuntu
# with nothing of ours: the .deb installs over 22.04 with its stated
# dependencies, the AppImage runs on 24.04 with no WebKit installed at
# all. Each is started on a virtual display and must open its window.
{ pkgs, linux }:
let
  vm = pkgs.vmTools;
  check =
    name: image: script:
    vm.runInLinuxImage (
      pkgs.stdenv.mkDerivation {
        name = "commonty-${name}-starts";
        diskImage = image;
        diskImageFormat = "qcow2";
        memSize = 4096;
        buildCommand = ''
          set -eu
          export HOME=/tmp/home XDG_RUNTIME_DIR=/tmp/run
          mkdir -p $HOME $XDG_RUNTIME_DIR
          Xvfb :9 -screen 0 1280x800x24 >/dev/null 2>&1 &
          export DISPLAY=:9
          sleep 2
          ${script}
          # the window, by its title, within half a minute
          for i in $(seq 30); do
            if xwininfo -root -tree | grep -q '"Commonty"'; then
              echo "window open after ''${i}s"
              mkdir -p $out
              xwininfo -root -tree | grep '"Commonty"' > $out/window
              exit 0
            fi
            kill -0 $app || { echo "the app exited"; cat /tmp/app.log; exit 1; }
            sleep 1
          done
          echo "no window"; cat /tmp/app.log; exit 1
        '';
      }
    );
  runtime = [
    "xvfb"
    "x11-utils"
  ];
in
{
  deb =
    check "deb"
      (vm.diskImageFuns.ubuntu2204x86_64 {
        # what the .deb's control file asks for, as apt would bring it
        extraPackages = runtime ++ [
          "libwebkit2gtk-4.1-0"
          "libgtk-3-0"
          "gstreamer1.0-plugins-base"
          "gstreamer1.0-plugins-good"
          "gstreamer1.0-libav"
        ];
      })
      ''
        dpkg -i ${linux}/*.deb
        # the card writer the installer runs, carried beside the app
        dpkg -L commonty | grep -q '/dd-hub-writer$' || { echo "the .deb left out dd-hub-writer"; exit 1; }
        commonty >/tmp/app.log 2>&1 &
        app=$!
      '';
  # An AppImage brings everything but what every desktop already has
  # (linuxdeploy's exclude list: the display server's and the GPU's
  # libraries, fonts); those, and no WebKit or GTK
  appimage =
    check "appimage"
      (vm.diskImageFuns.ubuntu2404x86_64 {
        extraPackages = runtime ++ [
          "libwayland-client0"
          "libwayland-cursor0"
          "libwayland-egl1"
          "libegl1"
          "libgl1"
          "libgbm1"
          "libfontconfig1"
          "libfreetype6"
          "libharfbuzz0b"
          "libx11-6"
          "libxcb1"
          "libxext6"
          "libxrender1"
          # the rest of what it does not carry (readelf over the AppImage):
          # all on every Ubuntu desktop
          "libasound2t64"
          "libdrm2"
          "libexpat1"
          "libfribidi0"
          "libusb-1.0-0"
          "libx11-xcb1"
          "libstdc++6"
          # what WebKit opens by name as it starts, and Mesa's drivers to
          # draw with no GPU (every desktop has a GL)
          "libgles2"
          "libegl-mesa0"
          "libgl1-mesa-dri"
        ];
      })
      ''
        cp ${linux}/*.AppImage /tmp/c.AppImage && chmod +x /tmp/c.AppImage
        # no fuse in the test machine: the AppImage unpacks itself instead
        APPIMAGE_EXTRACT_AND_RUN=1 /tmp/c.AppImage >/tmp/app.log 2>&1 &
        app=$!
      '';
}
