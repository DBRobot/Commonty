# Movies & TV (servers/media/web) in a browser, as the app serves it: the
# carried pages and webDist against the Mail test's made-up app and gate
# (mail-page/fake.py), whose box takes two seconds to start a film. A
# film's band starts it at the box, so Play does not wait, and closing
# the band or the player leaves nothing going there.
{ pkgs, web, ... }:
let
  python = pkgs.python3.withPackages (ps: [
    ps.playwright
    ps.cryptography
  ]);
in
pkgs.runCommand "media-page-e2e"
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
    python3 ${./mail-page/fake.py} ${../../app/web/_dd} 8792 ${web} > fake.log 2>&1 &
    fake=$!
    for i in $(seq 1 50); do curl -sf -o /dev/null http://127.0.0.1:8792/_dd/media && break; sleep 0.2; done
    status=0
    python3 ${./media-page/flow.py} 8792 $out || status=$?
    kill $fake 2>/dev/null || true
    if [ $status -ne 0 ]; then echo "--- fake log"; tail -20 fake.log; exit $status; fi
  ''
