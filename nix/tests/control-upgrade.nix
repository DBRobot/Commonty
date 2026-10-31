# The control plane moving from the release the fleet runs to this one
# while it decides (nix/tests/control-upgrade/run.py): two box validators,
# the witness and a box that only reads, all on the old dd-control and
# witness, switched to this release's one at a time, in two orders. The old
# programs are built from their commit, fetched from the forge's public
# door. No network, no vm.
#
# scenario "together": release 360's switch of 2026-10-24, the witness in
# the config and off, both boxes switched at once; the leased writers must
# run again within the lease's time and a margin (checks.control-together).
#
# scenario "signfail": node1's signatures fail (dd-control -test-sign-fail),
# first for 20 s, then for longer than a box counts as alive; the writers
# go to node2, which lacks their data, only while node1 is gone, and back
# to node1 once it signs again (checks.control-signfail).
#
# scenario "flap": the failover flapping of 2026-10-25/26. node1 holds the
# writers, node2 suits them better by score: a stall of agreement past the
# lease's time gives them back to node1 and nothing moves for minutes after;
# node1 drained, node2 takes them over with postgres restoring for a minute
# and does not refuse it; node2 drained and node1 refusing postgres, the
# plan still places it (checks.control-flap).
{
  pkgs,
  self,
  scenario ? "orders",
  ...
}:
let
  sys = pkgs.stdenv.hostPlatform.system;
  python = pkgs.python3.withPackages (ps: [ ps.cryptography ]);
  # release 370, what the fleet runs before this one (state machine 2)
  old =
    let
      src = pkgs.fetchgit {
        url = "https://code.commonty.org/david/commonty.git";
        rev = "489e6dfe07e7bc41f87af01b588277039682437d";
        sparseCheckout = [ "servers/system/control" ];
        hash = "sha256-UvLbiKoWogP2NxZh2bTaIz5HBK/gyTHzGUQYZvqT2iQ=";
      };
      control = "${src}/servers/system/control";
      dd-control = pkgs.buildGoModule {
        pname = "dd-control-old";
        version = "0.1.0";
        src = control;
        vendorHash = "sha256-PdyOvSGb4sSM/5lYK1O2WwwfGPpkWtGxAoJ2RfuzjRU=";
        subPackages = [ "cmd/dd-control" ];
        doCheck = false;
      };
    in
    {
      inherit dd-control;
      witness = pkgs.runCommand "dd-control-witness-old" { nativeBuildInputs = [ pkgs.go ]; } ''
        export HOME=$TMPDIR GOCACHE=$TMPDIR/cache GOFLAGS=-mod=vendor GOOS=js GOARCH=wasm CGO_ENABLED=0
        cp -r ${control} src && chmod -R u+w src && cd src
        cp -r ${dd-control.goModules} vendor
        mkdir -p $out
        go build -trimpath -ldflags="-s -w" -o $out/witness.wasm ./cmd/witness
      '';
    };
  new = self.packages.${sys};
in
pkgs.runCommand (if scenario == "orders" then "control-upgrade" else "control-${scenario}")
  {
    nativeBuildInputs = [
      python
      pkgs.wrangler
      pkgs.curl
    ];
  }
  ''
    export HOME=$TMPDIR WRANGLER_SEND_METRICS=false CI=1 CLOUDFLARE_CF_FETCH_ENABLED=false
    mkdir -p old new
    cp ${old.dd-control}/bin/dd-control old/dd-control
    cp ${old.witness}/witness.wasm old/witness.wasm
    cp ${new.dd-control}/bin/dd-control new/dd-control
    cp ${new.control-worker}/witness.wasm new/witness.wasm
    # the relay itself is this release's either way: what decides is the
    # witness inside it
    cp ${new.control-worker}/{worker.js,wasm_exec.js} .
    chmod -R u+w .
    SCENARIO=${scenario} PLACEMENT=${./control-upgrade/placement.json} python3 ${./control-upgrade/run.py}
    touch $out
  ''
