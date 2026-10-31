# Signing in once for every box, end to end: the real gate (as node1 runs
# it, with sign-in at the id Worker) and the real id Worker under wrangler,
# driven by a browser at their own https names - Playwright answers
# https://id.commonty.test from the Worker and https://home. and
# https://files.commonty.test from the gate, so the page, its origin
# checks and its __Host- cookies are as they are deployed. The made-up
# member (client/mail/tests/tester.json) answers the passkey prompt.
{ pkgs, self, ... }:
let
  system = pkgs.stdenv.hostPlatform.system;
  python = pkgs.python3.withPackages (ps: [
    ps.playwright
    ps.cryptography
  ]);
in
pkgs.runCommand "sso-e2e"
  {
    nativeBuildInputs = [
      python
      pkgs.wrangler
      pkgs.curl
      pkgs.jq
    ];
    PLAYWRIGHT_BROWSERS_PATH = pkgs.playwright-driver.browsers;
    PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
    FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
  }
  ''
    export HOME=$TMPDIR WRANGLER_SEND_METRICS=false CI=1 CLOUDFLARE_CF_FETCH_ENABLED=false
    tester=${../../client/mail/tests/tester.json}

    # the gate: the member's entry on file, the member list, sign-in at the id Worker
    mkdir -p gate/keys
    jq '.v5' $tester > gate/keys/tester.json
    VERIFY_ROLE=full VERIFY_DIR=$PWD/gate/keys VERIFY_DOMAIN=commonty.test \
      VERIFY_MEMBERS="$(jq -r '.members' $tester)" VERIFY_HOSTS=files \
      VERIFY_ID=https://id.commonty.test VERIFY_BIND=127.0.0.1:4181 \
      VERIFY_PAGES_DIR=${self.packages.${system}.pages} \
      ${self.packages.${system}.verify}/bin/verify > gate.log 2>&1 &
    gate=$!

    # the same, at the stage before: the Worker can be tried, nothing else moves
    mkdir -p trying/keys
    jq '.v5' $tester > trying/keys/tester.json
    VERIFY_ROLE=full VERIFY_DIR=$PWD/trying/keys VERIFY_DOMAIN=commonty.test \
      VERIFY_MEMBERS="$(jq -r '.members' $tester)" VERIFY_HOSTS=files \
      VERIFY_ID=https://id.commonty.test VERIFY_ID_STAGE=try VERIFY_BIND=127.0.0.1:4182 \
      VERIFY_PAGES_DIR=${self.packages.${system}.pages} \
      ${self.packages.${system}.verify}/bin/verify > trying.log 2>&1 &
    trying=$!

    # the id Worker, as the release uploads it
    mkdir w
    cp -rL --no-preserve=mode ${self.packages.${system}.id-worker}/. w/
    cat > w/wrangler.toml <<'TOML'
    name = "id-e2e"
    main = "worker.js"
    compatibility_date = "2026-05-01"
    no_bundle = true
    rules = [
      { type = "Text", globs = ["**/*.html", "**/*.css", "**/id.js", "**/*.svg"] },
      { type = "Data", globs = ["**/*.woff2"] },
      { type = "ESModule", globs = ["**/worker.js"] },
    ]
    [vars]
    DOMAIN = "commonty.test"
    TOML
    sed -i 's/^    //' w/wrangler.toml
    (cd w && wrangler dev --port 8788 --ip 127.0.0.1 > ../wrangler.log 2>&1) &
    worker=$!

    for u in http://127.0.0.1:4181/health http://127.0.0.1:4182/health http://127.0.0.1:8788/; do
      for i in $(seq 1 120); do curl -s -o /dev/null "$u" && break; sleep 0.5; done
    done

    status=0
    python3 ${./sso/flow.py} $tester || status=$?
    kill $gate $trying $worker 2>/dev/null || true
    if [ $status -ne 0 ]; then
      echo "--- gate"; tail -30 gate.log
      echo "--- worker"; tail -30 wrangler.log
      exit $status
    fi
    touch $out
  ''
