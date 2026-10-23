# Signing out everywhere, end to end (nix/tests/signout/run.py): three boxes,
# each a dd-control validator with a full gate reading the agreed identity
# state, and the witness under wrangler. A member signs in on every box,
# signs out everywhere from one, and the sessions end on every box: on one
# that was off at the time once it is back, and on one still running the
# old dd-control once it moves to this one. No network, no vm.
{ pkgs, self, ... }:
let
  sys = pkgs.stdenv.hostPlatform.system;
  python = pkgs.python3.withPackages (ps: [ ps.cryptography ]);
  # main before the sign-out: the dd-control the fleet runs now, built
  # from its commit, fetched from the forge's public door
  old =
    let
      src = pkgs.fetchgit {
        url = "https://code.commonty.org/david/commonty.git";
        rev = "3cb590ef137bb9dace923775452cadfd26914c04";
        sparseCheckout = [ "servers/system/control" ];
        hash = "sha256-yK5ySVxyZdXjMrXfZ6WR6DN5z7cvfKrbBGFVv/r14u0=";
      };
    in
    pkgs.buildGoModule {
      pname = "dd-control-before-signout";
      version = "0.1.0";
      src = "${src}/servers/system/control";
      vendorHash = "sha256-0Pw/n4fnU3oM71OMEp2MkneaDXRXEmILhvF4VKTuAJE=";
      subPackages = [ "cmd/dd-control" ];
      doCheck = false;
    };
  new = self.packages.${sys};
in
pkgs.runCommand "signout-e2e"
  {
    nativeBuildInputs = [
      python
      pkgs.wrangler
      pkgs.curl
      pkgs.biscuit-cli
    ];
  }
  ''
    export HOME=$TMPDIR WRANGLER_SEND_METRICS=false CI=1 CLOUDFLARE_CF_FETCH_ENABLED=false
    mkdir -p old w
    cp ${old}/bin/dd-control old/dd-control
    cp ${new.dd-control}/bin/dd-control dd-control
    cp ${new.verify}/bin/verify verify
    cp ${new.control-worker}/{worker.js,wasm_exec.js,witness.wasm} w/
    chmod -R u+w .
    echo "export default $(cat ${./control/placement.json});" > w/placement.js
    # a test fleet: three boxes that each vote, the witness, and dee, let in
    # by the release, whose root is a key on her computer
    python3 - <<'PY'
    import json, base64, os, subprocess, hashlib
    from cryptography.hazmat.primitives.asymmetric import ec, ed25519
    from cryptography.hazmat.primitives import serialization as S
    raw = lambda k: base64.b64encode(k.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)).decode()
    cfg = {"boxes": {}, "validators": [], "rules": {"leaseTtlMs": 15000, "goneAfterMs": 10000, "leases": {}}}
    for i, n in enumerate(["node1", "node2", "node3"], start=1):
        k = ec.generate_private_key(ec.SECP256R1())
        open(f"{n}.p8", "wb").write(k.private_bytes(S.Encoding.DER, S.PrivateFormat.PKCS8, S.NoEncryption()))
        cfg["boxes"][n] = {"id": 100 + i, "house": f"h{i}", "key": base64.b64encode(k.public_key().public_bytes(S.Encoding.DER, S.PublicFormat.SubjectPublicKeyInfo)).decode()}
        os.makedirs(f"st{i}", exist_ok=True)
        pub = subprocess.check_output(["./dd-control", "-print-validator-key", "-state", f"st{i}"]).decode().strip()
        cfg["validators"].append({"id": i, "name": n, "key": pub})
    w = ed25519.Ed25519PrivateKey.generate()
    cfg["validators"].append({"id": 4, "name": "cloudflare", "key": raw(w)})
    rel = ed25519.Ed25519PrivateKey.generate()
    dee = ed25519.Ed25519PrivateKey.generate()
    open("dee.key", "wb").write(dee.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption()))
    cfg["identity"] = {"release": raw(rel), "members": {"members": [hashlib.sha256(raw(dee).encode()).hexdigest()], "revoked": []}}
    json.dump(cfg, open("config.json", "w"))
    open("w/config.js", "w").write("export default " + json.dumps(cfg) + ";\n")
    open("w/.dev.vars", "w").write("WITNESS_KEY=" + base64.b64encode(w.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption())).decode() + "\n")
    PY
    cat > w/wrangler.toml <<'TOML'
    name = "signout-e2e"
    main = "worker.js"
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
    PAGES=${new.pages} PLACEMENT=${./control/placement.json} python3 ${./signout/run.py} || status=$?
    kill $worker 2>/dev/null || true
    if [ $status -ne 0 ]; then
      echo "--- relay"; tail -30 wrangler.log
      for i in 1 2 3; do echo "--- node$i"; tail -20 n$i.log; echo "--- gate$i"; tail -20 g$i.log 2>/dev/null; done
      exit $status
    fi
    touch $out
  ''
