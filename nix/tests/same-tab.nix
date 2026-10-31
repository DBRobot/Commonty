# Every link in the bar and every tile on the home page stays in the tab it
# was clicked in, on a box's pages and on the pages the app serves a
# browser (same-tab/flow.py). The pages and their scripts are the app's
# copies, which are the box's own byte for byte (checked here first).
{ pkgs, ... }:
let
  python = pkgs.python3.withPackages (ps: [ ps.playwright ]);
in
pkgs.runCommand "same-tab-e2e"
  {
    nativeBuildInputs = [ python ];
    PLAYWRIGHT_BROWSERS_PATH = pkgs.playwright-driver.browsers;
    PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
    FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
  }
  ''
    export HOME=$TMPDIR
    cmp ${../../servers/web-shared/shell.js} ${../../app/web/_dd/static/shell.js}
    cmp ${../../servers/home/web/pages/home.html} ${../../app/web/_dd/pages/home.html}
    python3 ${./same-tab/flow.py} ${../../app/web/_dd} ${../../servers/home/web}
    touch $out
  ''
