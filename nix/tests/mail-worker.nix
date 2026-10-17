# The mail Worker end to end: the very files a release uploads, run locally
# by wrangler (workerd), against a fake Cloudflare and directory, driven by
# a browser through the change-email pages and the line in Settings, then
# at its API with what it must refuse. A made-up member signs in
# (client/mail/tests/tester.json, made by client/mail/examples/fixture.rs). No network, no vm: under a
# minute, and cached by content like every other check. A box's mail
# (send.js) goes to wrangler's own stand-in for send_email, which writes
# each message to a file and names it in wrangler.log. Mail to a member's
# alias goes in at wrangler's /cdn-cgi/handler/email, and where it was sent
# on is read back from wrangler.log too.
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
    # the made-up member is the only one on the list, and the release key is
    # RFC 8032's first test key: the Worker decodes it, and grants nothing by it;
    # its aliases: security to it, and two the Worker must refuse (a
    # member's address already, "other", and one that is no alias) - fake.py
    # starts with a rule for a dropped one, "old", and other's own
    printf 'export const MEMBERS = %s;\nexport const RELEASE = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=";\nexport const BOXES = {"testbox": %s};\nexport const ALIASES = {"security": "tester", "other": "tester", "Bad.One": "tester"};\nexport const OWNER = "tester";\n' \
      "$(jq -c '.members' $tester)" "$(jq -c '.box_spki' $tester)" > w/fleet.js
    cat > w/wrangler.toml <<'TOML'
    name = "mail-e2e"
    main = "worker.js"
    compatibility_date = "2026-05-01"
    no_bundle = true
    rules = [
      { type = "Text", globs = ["**/*.html", "**/*.css", "**/change.js", "**/row.js", "**/passkey.js", "**/boxes.js", "**/aliases-page.js"] },
      { type = "Data", globs = ["**/*.woff2"] },
      { type = "CompiledWasm", globs = ["**/*.wasm"] },
      { type = "ESModule", globs = ["**/worker.js", "**/dd_mail.js", "**/fleet.js", "**/unlock.js", "**/leaving.js", "**/send.js", "**/aliases.js", "**/identity.js"] },
    ]
    kv_namespaces = [{ binding = "PINNED", id = "local" }]
    send_email = [{ name = "SEND", allowed_sender_addresses = ["noreply@commonty.test", "alerts@commonty.test"] }]
    [vars]
    DOMAIN = "commonty.test"
    WORKER = "mail-e2e"
    CF_API = "http://127.0.0.1:8899/client/v4"
    DIRECTORY = "http://127.0.0.1:8899/dir/"
    ORIGIN = "http://localhost:8787"
    SEND_PER_BOX = "6"
    SEND_PER_RCPT = "4"
    # a day is two seconds here, so an alias that turns off "after a day" can
    # be seen to (aliases.js)
    ALIAS_DAY = "2"
    TOML
    sed -i 's/^    //' w/wrangler.toml
    # the read key: 32 bytes of 5, as the fake directory expects (fake.py)
    printf 'SESSION_KEY=e2e-session-key-0123456789\nCF_TOKEN=fake\nDIRECTORY_KEY=BQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQU=\n' > w/.dev.vars

    # and two more of it reading who is a member from the agreed identity
    # state (docs/dev/identity-state.md, fake.py's /identity): agreed alone,
    # and shadow, which still goes by its own judgment
    for m in agreed:8790 shadow:8791; do
      d=w-''${m%%:*}
      cp -r w $d
      sed -i "s|^ORIGIN = .*|ORIGIN = \"http://localhost:''${m##*:}\"\nIDENTITY_FROM = \"''${m%%:*}\"\nIDENTITY_URL = \"http://127.0.0.1:8899/identity\"|; s|^name = .*|name = \"mail-e2e-''${m%%:*}\"|" $d/wrangler.toml
    done

    python3 $t/fake.py $tester > fake.log 2>&1 &
    fake=$!
    (cd w && wrangler dev --port 8787 --ip 127.0.0.1 > ../wrangler.log 2>&1) &
    worker=$!
    (cd w-agreed && wrangler dev --port 8790 --inspector-port 9240 --ip 127.0.0.1 > ../wrangler-agreed.log 2>&1) &
    worker_a=$!
    (cd w-shadow && wrangler dev --port 8791 --inspector-port 9241 --ip 127.0.0.1 > ../wrangler-shadow.log 2>&1) &
    worker_s=$!
    for port in 8787 8790 8791; do
      for i in $(seq 1 120); do
        curl -sf -o /dev/null http://127.0.0.1:$port/change && break
        sleep 0.5
      done
      curl -sf -o /dev/null http://127.0.0.1:$port/change || { cat wrangler*.log; exit 1; }
    done

    status=0
    DEBUG=pw:browser python3 $t/flow.py $tester 2> browser.log || status=$?
    [ $status -ne 0 ] || python3 $t/identity.py $PWD/wrangler-shadow.log || status=$?
    kill $worker $worker_a $worker_s $fake 2>/dev/null || true
    if [ $status -ne 0 ]; then
      echo "--- browser log"; grep -iE "crash|error|fatal|signal" browser.log | tail -20
      echo "--- worker log"; tail -40 wrangler.log
      echo "--- agreed worker log"; tail -20 wrangler-agreed.log
      echo "--- shadow worker log"; tail -20 wrangler-shadow.log
      exit $status
    fi
    touch $out
  ''
