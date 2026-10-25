# The relay without a working witness (servers/system/control/worker): the
# bundle this release builds, under wrangler. A witness that panics on boot,
# with or without a saved state, leaves /health and /relay answering, its
# health saying why, and /leases and /identity refused; a fresh object's
# witness starts, and so does one whose saved state does not hold
# together. The DNS and mail Workers bind the same object.
{ pkgs, self, ... }:
let
  sys = pkgs.stdenv.hostPlatform.system;
  python = pkgs.python3.withPackages (ps: [ ps.cryptography ]);
in
pkgs.runCommand "control-relay"
  {
    nativeBuildInputs = [
      python
      pkgs.wrangler
      pkgs.curl
    ];
  }
  ''
    export HOME=$TMPDIR WRANGLER_SEND_METRICS=false CI=1 CLOUDFLARE_CF_FETCH_ENABLED=false
    mkdir w && cp ${
      self.packages.${sys}.control-worker
    }/{worker.js,wasm_exec.js,witness.wasm} w/ && chmod u+w w/*
    echo "export default $(cat ${./control/placement.json});" > w/placement.js
    cp ${../../servers/system/dns/worker.js} dns.js
    cp ${../../client/mail/worker/identity.js} identity.js
    cat > w/front.js <<'JS'
    import relay, { Relay as Base } from './worker.js';
    // the test's hooks: a saved state put in place, or read back, before
    // the object has started its witness
    export class Relay extends Base {
      async fetch(req) {
        const u = new URL(req.url);
        if (u.pathname === '/test/saved') {
          if (req.method === 'PUT') { await this.ctx.storage.put('saved', await req.text()); return new Response('ok'); }
          return new Response((await this.ctx.storage.get('saved')) || "");
        }
        return super.fetch(req);
      }
    }
    export default {
      async fetch(req, env) {
        const u = new URL(req.url);
        // any object by name, as the DNS and mail Workers reach one
        const o = u.searchParams.get('o');
        if (o || u.pathname === '/leases' || u.pathname === '/identity') return env.RELAY.get(env.RELAY.idFromName(o || Base.NAME)).fetch(req);
        return relay.fetch(req, env);
      },
    };
    JS
    sed -i 's/^    //' w/front.js
    cat > w/wrangler.toml <<'TOML'
    name = "control-relay"
    main = "front.js"
    compatibility_date = "2026-05-01"
    no_bundle = true
    rules = [
      { type = "CompiledWasm", globs = ["**/*.wasm"], fallthrough = false },
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
    python3 ${./control-relay.py}
    touch $out
  ''
