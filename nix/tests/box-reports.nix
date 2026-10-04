# fleet/boxes/<box>/'s hardware files are what the box reported, signed by
# its TPM: nobody edits a box's hardware by hand (box-reports/check.py)
{ pkgs, ... }@args:
let
  src = pkgs.lib.fileset.toSource {
    root = ../..;
    fileset = pkgs.lib.fileset.unions [
      ../../fleet/boxes.json
      ../../fleet/boxes
    ];
  };
in
let
  # and the roles each box is given fit what it reported (role-fit.nix):
  # built with this check, which ci already runs
  roleFit = import ./role-fit.nix args;
in
pkgs.runCommand "box-reports" { nativeBuildInputs = [ (pkgs.python3.withPackages (p: [ p.cryptography ])) ]; } ''
  python3 ${./box-reports/check.py} ${src}
  test -e ${roleFit}
  touch $out
''
