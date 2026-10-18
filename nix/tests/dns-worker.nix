# The DNS Worker at its API: the very file a release uploads, run locally
# by wrangler (workerd) against a fake Cloudflare DNS, with two made-up
# boxes' keys. Then the box's own helper (nix/modules/gate/acme-ask.sh),
# signing with a key file as it signs with the TPM. No network, no vm.
{ pkgs, self, ... }:
let
  bundle = self.packages.${pkgs.stdenv.hostPlatform.system}.dns-worker;
  python = pkgs.python3.withPackages (ps: [ ps.cryptography ]);
in
pkgs.runCommand "dns-worker-check"
  {
    nativeBuildInputs = [
      python
      pkgs.wrangler
      pkgs.curl
      pkgs.jq
      pkgs.openssl
    ];
  }
  ''
    export HOME=$TMPDIR WRANGLER_SEND_METRICS=false CI=1 CLOUDFLARE_CF_FETCH_ENABLED=false
    t=${./dns-worker}
    for b in box1 box2; do
      openssl ecparam -name prime256v1 -genkey -noout -out $b.ec
      openssl pkcs8 -topk8 -nocrypt -in $b.ec -out $b.pem
    done
    spki() { openssl ec -in $1.pem -pubout -outform DER 2>/dev/null | base64 -w0; }
    mkdir w
    cp ${bundle}/worker.js ${bundle}/controller.js ${bundle}/identity.js ${bundle}/dd_mail.js ${bundle}/dd_mail_bg.wasm w/
    tester=${../../client/mail/tests/tester.json}
    cat > w/fleet.js <<EOF
    export const BOXES = {"box1": "$(spki box1)", "box2": "$(spki box2)"};
    export const NAMES = {"box1": ["commonty.test", "home.commonty.test", "files.commonty.test"], "box2": ["photos.commonty.test"]};
    export const RECORDS = {"zone": "commonty.test", "address": "100.64.0.2", "tunnel": "t1", "hosts": ["home", "accounts", "demo-files"], "open": true, "bare": true};
    export const MAIL = null;
    export const MEMBERS = $(jq -c '.members' $tester);
    export const RELEASE = "$(tr -d '[:space:]' < ${../../fleet/release.pub})";
    EOF
    cat > w/wrangler.toml <<'TOML'
    name = "dns-check"
    main = "worker.js"
    compatibility_date = "2026-05-01"
    no_bundle = true
    rules = [{ type = "ESModule", globs = ["**/*.js"] }, { type = "CompiledWasm", globs = ["**/*.wasm"] }]
    [vars]
    DOMAIN = "commonty.test"
    ZONE = "zone1"
    CF_API = "http://127.0.0.1:8899/client/v4"
    TOML
    sed -i 's/^    //' w/wrangler.toml w/fleet.js
    printf 'DNS_TOKEN=dns-token\n' > w/.dev.vars

    # the same Worker with the door shut
    cp -r w w2
    sed -i 's/"open": true/"open": false/' w2/fleet.js

    # and with failover: the names follow the leases the control plane's
    # agreed state has, and each box has a tunnel of its own
    cp -r w w3
    cat > w3/fleet.js <<EOF
    export const BOXES = {"box1": "$(spki box1)", "box2": "$(spki box2)"};
    export const NAMES = {"box1": [], "box2": []};
    export const RECORDS = {"zone": "commonty.test", "address": "100.64.0.2", "tunnel": "t1", "hosts": ["home"], "open": true, "bare": false, "home": "box1", "gateway": "headscale", "addresses": {"box1": "100.64.0.2", "box2": "100.64.0.5"}, "fleet": ["100.64.0.2", "100.64.0.5"], "follow": {"photos": "ente-museum"}};
    export const MAIL = {"zone": "commonty.test", "dkim": {"selector": "cm1", "p": "$(printf 'A%.0s' $(seq 1 270))"}, "dmarc": "v=DMARC1; p=none; adkim=s; aspf=r"};
    export const MEMBERS = $(jq -c '.members' $tester);
    EOF
    sed -i 's/^    //' w3/fleet.js
    cat >> w3/wrangler.toml <<'TOML'
    CONTROL_URL = "http://127.0.0.1:8899/control"
    ACCOUNT = "acct1"
    TOML
    sed -i 's/^    //' w3/wrangler.toml
    printf 'DNS_TOKEN=dns-token\nTUNNEL_TOKEN=tunnel-token\n' > w3/.dev.vars

    # and with who is a member read from the agreed identity state
    # (docs/dev/identity-state.md), against a fake of its own
    cp -r w w4
    sed -i 's|^CF_API = .*|CF_API = "http://127.0.0.1:8898/client/v4"\nIDENTITY_FROM = "agreed"\nIDENTITY_URL = "http://127.0.0.1:8898/identity"|; s|^name = .*|name = "dns-check-agreed"|' w4/wrangler.toml

    python3 $t/fake.py > fake.log 2>&1 &
    fake=$!
    PORT=8898 TESTER=$tester python3 $t/fake.py > fake4.log 2>&1 &
    fake4=$!
    (cd w && wrangler dev --test-scheduled --port 8787 --ip 127.0.0.1 > ../wrangler.log 2>&1) &
    worker=$!
    (cd w2 && wrangler dev --test-scheduled --port 8788 --inspector-port 9230 --ip 127.0.0.1 > ../wrangler2.log 2>&1) &
    worker2=$!
    (cd w3 && wrangler dev --test-scheduled --port 8789 --inspector-port 9231 --ip 127.0.0.1 > ../wrangler3.log 2>&1) &
    worker3=$!
    (cd w4 && wrangler dev --test-scheduled --port 8792 --inspector-port 9232 --ip 127.0.0.1 > ../wrangler4.log 2>&1) &
    worker4=$!
    for p in 8787 8788 8789 8792; do
      for i in $(seq 1 120); do
        curl -s -o /dev/null http://127.0.0.1:$p/ && break
        sleep 0.5
      done
    done

    # the helper as the box runs it: its names, its key, this Worker
    {
      echo "export BOX=box1 WORKER=http://127.0.0.1:8787 KEYS=/nonexistent SOFT_KEY=$PWD/box1.pem"
      echo 'export NAMES=$(printf "%s\n" commonty.test home.commonty.test files.commonty.test)'
      cat ${../modules/gate/acme-ask.sh}
    } > helper.sh
    # and the box's door helper (nix/modules/gate/door-ask.sh), as box1
    {
      echo "export BOX=box1 WORKER=http://127.0.0.1:8789 KEYS=/nonexistent SOFT_KEY=$PWD/box1.pem"
      cat ${../modules/gate/box-sign.sh} ${../modules/gate/door-ask.sh}
    } > door.sh

    status=0
    python3 $t/flow.py $PWD/helper.sh $PWD/door.sh || status=$?
    [ $status -ne 0 ] || python3 $t/controller.py $tester || status=$?
    [ $status -ne 0 ] || AGREED=1 WORKER=http://127.0.0.1:8792 FAKE=http://127.0.0.1:8898 python3 $t/controller.py $tester || status=$?
    kill $worker $worker2 $worker3 $worker4 $fake $fake4 2>/dev/null || true
    if [ $status -ne 0 ]; then
      echo "--- worker log"; tail -40 wrangler.log
      echo "--- second worker log"; tail -20 wrangler2.log
      echo "--- third worker log"; tail -30 wrangler3.log
      echo "--- fake log"; tail -40 fake.log
      echo "--- agreed worker log"; tail -20 wrangler4.log
      exit $status
    fi
    touch $out
  ''
