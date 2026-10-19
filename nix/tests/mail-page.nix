# The Mail page (servers/mail/web) end to end in a browser: the page's
# files exactly as the app carries them (app/web/_dd), against a made-up
# app and box (mail-page/fake.py) that lock the test's mail with an HPKE of
# their own, and the member's Files library kept as the gate keeps it. No
# network, no vm. Screenshots of each look are the output.
{ pkgs, web, ... }:
let
  python = pkgs.python3.withPackages (ps: [
    ps.playwright
    ps.cryptography
  ]);
in
pkgs.runCommand "mail-page-e2e"
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
    t=${./mail-page}
    mkdir -p $out
    python3 $t/fake.py ${../../app/web/_dd} 8790 ${web} > fake.log 2>&1 &
    fake=$!
    for i in $(seq 1 50); do curl -sf -o /dev/null http://127.0.0.1:8790/_dd/mail && break; sleep 0.2; done
    status=0
    python3 $t/flow.py 8790 $out || status=$?
    kill $fake 2>/dev/null || true
    if [ $status -ne 0 ]; then echo "--- fake log"; tail -20 fake.log; exit $status; fi
  ''
