# The control plane end to end (nix/tests/control/run.py): the relay bundle
# this release builds, under wrangler, three dd-control validators, and a
# gate on each box keeping the directory as the agreed log. No network, no
# vm.
{ pkgs, self, ... }:
let
  sys = pkgs.stdenv.hostPlatform.system;
  python = pkgs.python3.withPackages (ps: [ ps.cryptography ]);
in
pkgs.runCommand "control-e2e"
  {
    nativeBuildInputs = [
      python
      pkgs.wrangler
      pkgs.curl
      pkgs.go_1_26
    ];
  }
  ''
    export HOME=$TMPDIR WRANGLER_SEND_METRICS=false CI=1 CLOUDFLARE_CF_FETCH_ENABLED=false
    # every package's own tests first (the state machine, the validators,
    # placement, the wire): this is the control plane's own check, the one
    # a change to its insides runs (nix/tests/ci-select/interfaces.py)
    cp -r ${../../servers/system/control} src && chmod -R u+w src
    cp -r ${self.packages.${sys}.dd-control.goModules} src/vendor
    (cd src && GOFLAGS=-mod=vendor GOCACHE=$TMPDIR/go-cache CGO_ENABLED=0 go test ./...)
    (cd src && GOFLAGS=-mod=vendor GOCACHE=$TMPDIR/go-cache CGO_ENABLED=0 go test -tags testswitches ./state/... ./cmd/...)
    mkdir w && cp ${self.packages.${sys}.control-worker-test}/{worker.js,wasm_exec.js,witness.wasm} w/
    # the test fleet's placement inputs, for the witness as for the boxes
    echo "export default $(cat ${./control/placement.json});" > w/placement.js
    # and /leases, which only the DNS Worker reaches (by its binding), here
    # through a front of the test's own
    cat > w/front.js <<'JS'
    import relay, { Relay } from './worker.js';
    export { Relay };
    export default {
      async fetch(req, env) {
        if (new URL(req.url).pathname === '/leases') return env.RELAY.get(env.RELAY.idFromName(Relay.NAME)).fetch(req);
        return relay.fetch(req, env);
      },
    };
    JS
    sed -i 's/^    //' w/front.js
    cp ${self.packages.${sys}.dd-control-test}/bin/dd-control . && chmod u+w w/*
    # each box's gate, for the directory over the agreed log
    cp ${self.packages.${sys}.verify}/bin/verify .
    # a test fleet: three boxes that each vote, and the witness
    python3 - <<'PY'
    import json, base64, os, subprocess
    from cryptography.hazmat.primitives.asymmetric import ec, ed25519
    from cryptography.hazmat.primitives import serialization as S
    cfg = {"boxes": {}, "validators": [], "rules": {"leaseTtlMs": 15000, "goneAfterMs": 10000, "leases": {"svc/db": {"eligible": ["node2", "node3"]}}}}
    for i, n in enumerate(["node1", "node2", "node3"], start=1):
        k = ec.generate_private_key(ec.SECP256R1())
        open(f"{n}.p8", "wb").write(k.private_bytes(S.Encoding.DER, S.PrivateFormat.PKCS8, S.NoEncryption()))
        cfg["boxes"][n] = {"id": 100 + i, "house": f"h{i}", "key": base64.b64encode(k.public_key().public_bytes(S.Encoding.DER, S.PublicFormat.SubjectPublicKeyInfo)).decode()}
        os.makedirs(f"st{i}", exist_ok=True)
        pub = subprocess.check_output(["./dd-control", "-print-validator-key", "-state", f"st{i}"]).decode().strip()
        cfg["validators"].append({"id": i, "name": n, "key": pub})
    w = ed25519.Ed25519PrivateKey.generate()
    cfg["validators"].append({"id": 4, "name": "cloudflare", "key": base64.b64encode(w.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)).decode()})
    # the identity judge's part: a release key, and nobody on the list
    rel = ed25519.Ed25519PrivateKey.generate()
    cfg["identity"] = {"release": base64.b64encode(rel.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)).decode(), "members": {"members": [], "revoked": []}}
    # and its private half, for the relay's copy of the release: dd signs
    # with it from a key file, as on the laptop
    fd = os.open("keys.json", os.O_WRONLY | os.O_CREAT, 0o600)
    os.write(fd, json.dumps({"release-key": base64.b64encode(rel.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption())).decode()}).encode())
    os.close(fd)
    # a member's remote computers: cara, let in by the release, whose box
    # node3 is; node2 hosts anyone's, node1 none
    import hashlib
    cara = ed25519.Ed25519PrivateKey.generate()
    open("cara.key", "wb").write(cara.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption()))
    croot = base64.b64encode(cara.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)).decode()
    cfg["identity"]["members"]["members"].append(hashlib.sha256(croot.encode()).hexdigest())
    cfg["computers"] = {"memory": 16384, "threads": 8, "disk": 200, "portBase": 27200, "portCount": 8, "owners": ["cara"]}
    cfg["rules"]["computerHosts"] = {"node2": "", "node3": "cara"}
    # the stranger here has no TPM; a join's proof of one is join-attest's
    cfg["attest"] = {"test": {"unattestedJoin": True}}
    json.dump(cfg, open("config.json", "w"))
    open("w/config.js", "w").write("export default " + json.dumps(cfg) + ";\n")
    open("w/.dev.vars", "w").write("WITNESS_KEY=" + base64.b64encode(w.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption())).decode() + "\n")
    PY
    cat > w/wrangler.toml <<'TOML'
    name = "control-e2e"
    main = "front.js"
    compatibility_date = "2026-05-01"
    no_bundle = true
    rules = [
      { type = "CompiledWasm", globs = ["**/*.wasm"] },
      { type = "ESModule", globs = ["**/*.js"] },
    ]
    [[durable_objects.bindings]]
    name = "RELAY"
    class_name = "Relay"
    [[migrations]]
    tag = "v1"
    new_sqlite_classes = ["Relay"]
    TOML
    sed -i 's/^    //' w/wrangler.toml
    (cd w && wrangler dev --port 8789 --ip 127.0.0.1 > ../wrangler.log 2>&1) &
    worker=$!
    for i in $(seq 1 120); do curl -sf -o /dev/null http://127.0.0.1:8789/health && break; sleep 0.5; done
    curl -sf -o /dev/null http://127.0.0.1:8789/health || { cat wrangler.log; exit 1; }
    status=0
    DD=${self.packages.${sys}.dd}/bin/dd PLACEMENT=${./control/placement.json} python3 ${./control/run.py} || status=$?
    kill $worker 2>/dev/null || true
    if [ $status -ne 0 ]; then
      echo "--- relay"; tail -30 wrangler.log; for i in 1 2 3; do echo "--- node$i"; tail -15 n$i.log; echo "--- gate$i"; tail -15 g$i.log 2>/dev/null; done
      exit $status
    fi
    touch $out
  ''
