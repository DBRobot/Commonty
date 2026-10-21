# The test chooser (ciselect.py) on made-up changes against covers.json:
# docs alone run nothing, a test's own file runs that test, a lockfile
# bump or a path nothing claims runs everything (edges.py). Seconds.
{ pkgs, ... }:
pkgs.runCommand "ci-select" { nativeBuildInputs = [ pkgs.python3 ]; } ''
  python3 ${./.}/edges.py ${../../..}
  touch $out
''
