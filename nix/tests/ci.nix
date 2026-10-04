# The ci workflow against the flake, which yaml cannot read: its vm matrix
# names exactly the tests the flake runs, and ci_ok - the one check main's
# rule asks for - waits on every other job, so no job can fail without
# blocking the merge. And the app's release build (.github/workflows/
# release.yml) compiles with the Rust the flake does, so what CI checked is
# what people download.
{
  pkgs,
  lib,
  vmTests,
  ...
}:
let
  workflow = ../../.forgejo/workflows/entry_point.yml;
  release = ../../.github/workflows/release.yml;
  rust = pkgs.rustc.version;
  wanted = builtins.toJSON (lib.sort builtins.lessThan vmTests);
in
pkgs.runCommand "ci-matches-the-flake" { nativeBuildInputs = [ pkgs.yq-go ]; } ''
  have=$(yq -o=json -I=0 '.jobs.vm_tests.strategy.matrix.test | sort' ${workflow})
  if [ "$have" != '${wanted}' ]; then
    echo "ci matrix $have != flake vm tests ${wanted}" >&2
    exit 1
  fi
  jobs=$(yq -o=json -I=0 '[.jobs | keys | .[] | select(. != "ci_ok")] | sort' ${workflow})
  gate=$(yq -o=json -I=0 '.jobs.ci_ok.needs | sort' ${workflow})
  if [ "$jobs" != "$gate" ]; then
    echo "ci_ok waits on $gate, but the jobs are $jobs" >&2
    exit 1
  fi
  if ! yq '.jobs.ci_ok.if' ${workflow} | grep -q 'always()'; then
    echo "ci_ok must run whatever its jobs did: if: always()" >&2
    exit 1
  fi
  others=$(yq -o=json -I=0 '[.. | select(has("toolchain")) | .toolchain] | unique' ${release})
  if [ "$others" != '["${rust}"]' ]; then
    echo "release.yml builds the app with Rust $others; the flake compiles with ${rust}" >&2
    exit 1
  fi
  echo ok > $out
''
