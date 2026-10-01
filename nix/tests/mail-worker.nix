# The mail Worker end to end: the very files a release uploads, run locally
# by wrangler (workerd), against a fake Cloudflare and directory, driven by
# a browser through the change-email pages and the line in Settings, then
# at its API with what it must refuse. A made-up member signs in
# (client/mail/tests/tester.json, made by client/mail/examples/fixture.rs). No network, no vm: under a
# minute, and cached by content like every other check.
{ pkgs, self, ... }:
let
  bundle = self.packages.${pkgs.stdenv.hostPlatform.system}.mail-worker;
  python = pkgs.python3.withPackages (ps: [
    ps.playwright
    ps.cryptography
  ]);
in
pkgs.runCommand "mail-worker-e2e"
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
    # no network in here: wrangler would fetch request metadata it does not need
    export HOME=$TMPDIR WRANGLER_SEND_METRICS=false CI=1 CLOUDFLARE_CF_FETCH_ENABLED=false
    t=${./mail-worker}
    tester=${../../client/mail/tests/tester.json}
    mkdir w
    cp -rL --no-preserve=mode ${bundle}/. w/
    # the made-up member is the only one on the list
    printf 'export const MEMBERS = %s;\nexport const RELEASE = "unused";\n' \
      "$(jq -c '.members' $tester)" > w/fleet.js
    cat > w/wrangler.toml <<'TOML'
    name = "mail-e2e"
    main = "worker.js"
    compatibility_date = "2026-05-01"
    no_bundle = true
    rules = [
      { type = "Text", globs = ["**/*.html", "**/*.css", "**/change.js", "**/row.js", "**/passkey.js"] },
      { type = "Data", globs = ["**/*.woff2"] },
      { type = "CompiledWasm", globs = ["**/*.wasm"] },
      { type = "ESModule", globs = ["**/worker.js", "**/dd_mail.js", "**/fleet.js"] },
    ]
    kv_namespaces = [{ binding = "PINNED", id = "local" }]
    [vars]
    DOMAIN = "commonty.test"
    CF_API = "http://127.0.0.1:8899/client/v4"
    DIRECTORY = "http://127.0.0.1:8899/dir/"
    ORIGIN = "http://localhost:8787"
    TOML
    sed -i 's/^    //' w/wrangler.toml
    printf 'SESSION_KEY=e2e-session-key-0123456789\nCF_TOKEN=fake\n' > w/.dev.vars

    python3 $t/fake.py $tester > fake.log 2>&1 &
    fake=$!
    (cd w && wrangler dev --port 8787 --ip 127.0.0.1 > ../wrangler.log 2>&1) &
    worker=$!
    for i in $(seq 1 120); do
      curl -sf -o /dev/null http://127.0.0.1:8787/change && break
      sleep 0.5
    done
    curl -sf -o /dev/null http://127.0.0.1:8787/change || { cat wrangler.log; exit 1; }

    status=0
    DEBUG=pw:browser python3 $t/flow.py 2> browser.log || status=$?
    kill $worker $fake 2>/dev/null || true
    if [ $status -ne 0 ]; then
      echo "--- browser log"; grep -iE "crash|error|fatal|signal" browser.log | tail -20
      echo "--- worker log"; tail -40 wrangler.log
      exit $status
    fi
    touch $out
  ''
