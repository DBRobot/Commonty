# The Files page (servers/files/web) in a browser, as the app serves it: the
# carried pages and webDist against the Mail test's made-up app and gate
# (mail-page/fake.py), which keeps the member's library as the gate does.
# Search across the library, folder sizes, a folder out as a zip, folders
# in, an address that opens a file, and Scan to PDF with its words read.
{ pkgs, web, ... }:
let
  python = pkgs.python3.withPackages (ps: [
    ps.playwright
    ps.cryptography
  ]);
in
pkgs.runCommand "files-page-e2e"
  {
    nativeBuildInputs = [
      python
      pkgs.curl
    ];
    PLAYWRIGHT_BROWSERS_PATH = pkgs.playwright-driver.browsers;
    PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
    FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
  }
  ''
    export HOME=$TMPDIR
    mkdir -p $out
    python3 ${./mail-page/fake.py} ${../../app/web/_dd} 8791 ${web} > fake.log 2>&1 &
    fake=$!
    for i in $(seq 1 50); do curl -sf -o /dev/null http://127.0.0.1:8791/_dd/files && break; sleep 0.2; done
    status=0
    python3 ${./files-page/flow.py} 8791 $out || status=$?
    kill $fake 2>/dev/null || true
    if [ $status -ne 0 ]; then echo "--- fake log"; tail -20 fake.log; exit $status; fi
  ''
