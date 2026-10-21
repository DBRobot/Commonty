# The ci workflow against the flake, which yaml cannot read: its vm matrix
# is the tests files_modified chose, from the flake's own list as the
# chooser reads it (nix/tests/ci-select), and ci_ok - the one check main's
# rule asks for - waits on every other job, so no job can fail without
# blocking the merge.
{
  pkgs,
  lib,
  vmTests,
  ...
}:
let
  workflow = ../../.forgejo/workflows/entry_point.yml;
  wanted = builtins.toJSON (lib.sort builtins.lessThan vmTests);
  matrix = "\${{ fromJSON(needs.files_modified.outputs.vm_tests) }}";
in
pkgs.runCommand "ci-matches-the-flake"
  {
    nativeBuildInputs = [
      pkgs.yq-go
      pkgs.python3
    ];
  }
  ''
    have=$(python3 ${./ci-select/ciselect.py} matches ${../../flake.nix} | tr -d ' ')
    if [ "$have" != '${wanted}' ]; then
      echo "the test chooser reads $have from flake.nix, the flake runs ${wanted}" >&2
      exit 1
    fi
    matrix=$(yq '.jobs.vm_tests.strategy.matrix.test' ${workflow})
    if [ "$matrix" != ${lib.escapeShellArg matrix} ]; then
      echo "the vm matrix is $matrix, not the tests files_modified chose" >&2
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
    echo ok > $out
  ''
