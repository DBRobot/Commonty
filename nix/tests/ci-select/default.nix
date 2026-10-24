# The test chooser (ciselect.py) on made-up changes against covers.json:
# docs alone run nothing, a test's own file runs that test, a lockfile
# bump or a path nothing claims runs everything, a component's insides
# with its face held run its own checks alone (edges.py); and the face
# readers on small made-up trees (faces_test.py). Seconds.
{ pkgs, self, ... }:
let
  sys = pkgs.stdenv.hostPlatform.system;
in
pkgs.runCommand "ci-select"
  {
    nativeBuildInputs = [
      pkgs.python3
      pkgs.go_1_26
      self.packages.${sys}.apidiff
      self.packages.${sys}.goface
    ];
  }
  ''
    export HOME=$TMPDIR GOCACHE=$TMPDIR/go-cache GOFLAGS=-mod=mod GOPROXY=off GOTOOLCHAIN=local
    python3 ${./.}/edges.py ${../../..}
    python3 ${./.}/faces_test.py
    touch $out
  ''
