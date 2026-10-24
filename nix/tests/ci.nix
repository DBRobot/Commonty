# The ci workflow against the flake, which yaml cannot read: its vm matrix
# is the tests files_modified chose, from the flake's own list as the
# chooser reads it (nix/tests/ci-select), each browser and Worker job's
# matrix is its group's choice, the installer jobs follow the installer's
# choice, and ci_ok - the one check main's
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
    # each browser and Worker job the chooser names exists, and runs the
    # checks it chose for that job
    for g in $(python3 ${./ci-select/ciselect.py} groups | tr -d '[]",' ); do
      want="\''${{ fromJSON(needs.files_modified.outputs.$g) }}"
      have=$(yq ".jobs.$g.strategy.matrix.check" ${workflow})
      if [ "$have" != "$want" ]; then
        echo "job $g's matrix is '$have', not the checks files_modified chose for it" >&2
        exit 1
      fi
      if ! yq ".jobs.$g.if" ${workflow} | grep -qF "outputs.$g != '[\"none\"]'"; then
        echo "job $g must skip when files_modified chose none of its checks (its list is [\"none\"])" >&2
        exit 1
      fi
    done
    if [ "$(yq ".jobs.installer_agrees.if" ${workflow})" != "\''${{ needs.files_modified.outputs.installer == 'true' }}" ]; then
      echo "job installer_agrees must run when files_modified chose the installer, and only then" >&2
      exit 1
    fi
    # build_installer always starts (a skipped matrix job left ci_ok blocked),
    # every step of it only when the installer is chosen
    if [ "$(yq ".jobs.build_installer.if" ${workflow})" != "null" ]; then
      echo "job build_installer must not be skipped whole: its steps carry the choice" >&2
      exit 1
    fi
    if [ "$(yq '[.jobs.build_installer.steps[] | select(.if != "failure()") | .if] | unique | .[]' ${workflow})" != "\''${{ needs.files_modified.outputs.installer == 'true' }}" ]; then
      echo "every step of build_installer must run when files_modified chose the installer, and only then" >&2
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
