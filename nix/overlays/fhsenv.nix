# nixpkgs' FHS rootfs builder picks between files of equal priority in a
# HashMap's order, which Rust seeds at random per process: two builds of one
# derivation link different copies of a library (steam-run's lib32 came out
# with one gmp on node1 and another on node2). The boxes vouch for a release
# by rebuilding it (dd-attest), so this has to come out the same every time;
# the tie goes to the lower path.
final: prev: {
  buildFHSEnvBubblewrap = prev.callPackage (prev.applyPatches {
    name = "build-fhsenv-bubblewrap";
    src = "${prev.path}/pkgs/build-support/build-fhsenv-bubblewrap";
    patches = [ ./fhsenv-tie.patch ];
  }) { };
  buildFHSEnv = final.buildFHSEnvBubblewrap;
}
