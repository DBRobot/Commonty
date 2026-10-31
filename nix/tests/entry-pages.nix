# The pages that read a member's signed entry (Settings' Profile and
# Devices, through shell.js entryOf) with it as the directory gives it: a
# v2 envelope, and a v1 entry as before (entry-pages/flow.py). The app's
# copies, which are the box's own byte for byte, and the real wasm.
{ pkgs, self, ... }:
let
  python = pkgs.python3.withPackages (ps: [
    ps.playwright
    ps.cryptography
  ]);
  web = self.packages.${pkgs.stdenv.hostPlatform.system}.web;
in
pkgs.runCommand "entry-pages-e2e"
  {
    nativeBuildInputs = [ python ];
    PLAYWRIGHT_BROWSERS_PATH = pkgs.playwright-driver.browsers;
    PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
    FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
  }
  ''
    export HOME=$TMPDIR
    cmp ${../../servers/web-shared/kit.js} ${../../app/web/_dd/static/kit.js}
    cmp ${../../servers/fleet/web/devices.js} ${../../app/web/_dd/static/devices.js}
    python3 ${./entry-pages/flow.py} ${../../app/web/_dd} ${web}
    touch $out
  ''
