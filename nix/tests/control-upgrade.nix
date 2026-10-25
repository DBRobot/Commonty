# The control plane moving from release 359 to this one while it decides
# (nix/tests/control-upgrade/run.py): two box validators, the witness and a
# box that only reads, all on release 359's dd-control and witness, switched
# to this release's one at a time, in two orders. Release 359's programs
# are built from its commit, fetched from the forge's public door. No
# network, no vm.
#
# scenario "together": release 360's switch of 2026-10-24, the witness in
# the config and off, both boxes switched at once; the leased writers must
# run again within the lease's time and a margin (checks.control-together).
{
  pkgs,
  self,
  scenario ? "orders",
  ...
}:
let
  sys = pkgs.stdenv.hostPlatform.system;
  python = pkgs.python3.withPackages (ps: [ ps.cryptography ]);
  # hotfix-355's head: release 359, what the fleet runs before this one
  old =
    let
      src = pkgs.fetchgit {
        url = "https://code.commonty.org/david/commonty.git";
        rev = "40c76145d806b6ec28704fe7ec6d2a66d841cd96";
        sparseCheckout = [ "servers/system/control" ];
        hash = "sha256-BYxms7+ABkfZCfa5JT05k2oGQ+BhpKA1tsDDXecxvGo=";
      };
      control = "${src}/servers/system/control";
      dd-control = pkgs.buildGoModule {
        pname = "dd-control-359";
        version = "0.1.0";
        src = control;
        vendorHash = "sha256-sYPGMtJetj/Ou1Q52s2tfRJkNcaiPlhuqIiq/H/1UjY=";
        subPackages = [ "cmd/dd-control" ];
        doCheck = false;
      };
    in
    {
      inherit dd-control;
      witness = pkgs.runCommand "dd-control-witness-359" { nativeBuildInputs = [ pkgs.go ]; } ''
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
