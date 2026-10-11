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
    cp ${bundle}/worker.js w/
    cat > w/fleet.js <<EOF
    export const BOXES = {"box1": "$(spki box1)", "box2": "$(spki box2)"};
    export const NAMES = {"box1": ["commonty.test", "home.commonty.test", "files.commonty.test"], "box2": ["photos.commonty.test"]};
    export const RECORDS = {"zone": "commonty.test", "address": "100.64.0.2", "tunnel": "t1", "hosts": ["home", "accounts", "demo-files"], "open": true, "bare": true};
    EOF
    cat > w/wrangler.toml <<'TOML'
    name = "dns-check"
    main = "worker.js"
    compatibility_date = "2026-05-01"
    no_bundle = true
    rules = [{ type = "ESModule", globs = ["**/*.js"] }]
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

    python3 $t/fake.py > fake.log 2>&1 &
    fake=$!
    (cd w && wrangler dev --test-scheduled --port 8787 --ip 127.0.0.1 > ../wrangler.log 2>&1) &
    worker=$!
    (cd w2 && wrangler dev --test-scheduled --port 8788 --inspector-port 9230 --ip 127.0.0.1 > ../wrangler2.log 2>&1) &
    worker2=$!
    for p in 8787 8788; do
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

    status=0
    python3 $t/flow.py $PWD/helper.sh || status=$?
    kill $worker $worker2 $fake 2>/dev/null || true
    if [ $status -ne 0 ]; then
      echo "--- worker log"; tail -40 wrangler.log
      echo "--- second worker log"; tail -20 wrangler2.log
      echo "--- fake log"; tail -40 fake.log
      exit $status
    fi
    touch $out
  ''
