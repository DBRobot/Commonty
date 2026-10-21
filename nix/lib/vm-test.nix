# A VM test is two files: <name>.nix says which boxes and what the script
# needs to know; <name>.py is the scenario. The Python gets one name, `nix`,
# a dict of the values the Nix side computed (store paths, box names), so
# nothing from Nix is interpolated into it and the file reads as Python.
{
  pkgs,
  lib,
  vmTests ? [ ],
  boxNames ? [ ],
}:
name: test:
let
  script = ../tests/${name}.py;
  # `helpers = true;` gives the script nix/tests/lib's modules, as a package
  # of the driver's own python (so its type check knows them too): `from waits
  # import start_fresh` (docs/dev/ci.md). Only for a test that asks, so the
  # others' drivers, and so their builds, stay as they are
  helpers =
    p:
    p.toPythonModule (
      pkgs.runCommand "dd-test-helpers" { } ''
        mkdir -p $out/${p.python.sitePackages}
        cp ${../tests/lib}/*.py $out/${p.python.sitePackages}/
      ''
    );
  body = builtins.readFile script;
  withEnv = env: "nix = ${builtins.toJSON env}\n" + body;
  testScript =
    if !(test ? scriptEnv) then
      body
    # a scriptEnv that needs the nodes is given them by the driver, which
    # passes a function only the arguments it names
    else if builtins.isFunction test.scriptEnv then
      ({ nodes, ... }: withEnv (test.scriptEnv { inherit nodes; }))
    else
      withEnv test.scriptEnv;
in
pkgs.testers.runNixOSTest (
  (builtins.removeAttrs test [
    "scriptEnv"
    "helpers"
  ])
  // lib.optionalAttrs (builtins.pathExists script) { inherit testScript; }
  // lib.optionalAttrs (test.helpers or false) {
    extraPythonPackages = p: [ (helpers p) ] ++ (test.extraPythonPackages or (_: [ ])) p;
  }
  // {
    # A test VM's store is the host's whole store under an overlay whose
    # writable half is a tmpfs of a gigabyte or two, far below a box's
    # min-free (roles/core.nix): the first store write of any nix in the VM
    # collected garbage first, and every host path the VM had not registered
    # was garbage, removed one walk at a time over 9p. On node1's store that
    # took from four minutes to past the joined-box test's fifteen
    # (2026-10-21). Nothing in a test frees store space on purpose.
    defaults = {
      imports = [ (test.defaults or { }) ];
      nix.settings.min-free = lib.mkForce 0;
    };
    # every box gets the script helper the real boxes get (flake.nix); the
    # test's other node settings stay
    node = (test.node or { }) // {
      specialArgs = (test.node.specialArgs or { }) // {
        inherit vmTests boxNames;
        ddScript = import ./script.nix { inherit lib; };
      };
    };
  }
)
