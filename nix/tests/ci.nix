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
    have=$(python3 ${./ci-select}/ciselect.py matches ${./registry.nix} | tr -d ' ')
    if [ "$have" != '${wanted}' ]; then
      echo "the test chooser reads $have from nix/tests/registry.nix, the flake runs ${wanted}" >&2
      exit 1
    fi
    matrix=$(yq '.jobs.vm_tests.strategy.matrix.test' ${workflow})
    if [ "$matrix" != ${lib.escapeShellArg matrix} ]; then
      echo "the vm matrix is $matrix, not the tests files_modified chose" >&2
      exit 1
    fi
    # each browser and Worker job the chooser names exists, and runs the
    # checks it chose for that job
    for g in $(python3 ${./ci-select}/ciselect.py groups | tr -d '[]",' ); do
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
    # a job whose runs-on comes from its matrix is never skipped whole: the
    # forge then left ci_ok waiting forever (run 857); its steps skip instead
    for j in build_systems build_installer; do
      case "$(yq ".jobs.$j.if" ${workflow})" in
        null | "\''${{ !cancelled() }}") ;;
        *) echo "job $j has runs-on from its matrix: skip its steps, never the whole job" >&2; exit 1 ;;
      esac
    done
    if ! yq '.jobs.build_installer.steps[0].if' ${workflow} | grep -qF "outputs.installer == 'true'"; then
      echo "build_installer's steps must run when files_modified chose the installer, and only then" >&2
      exit 1
    fi
    if [ "$(yq '.jobs.installer_agrees.if' ${workflow})" != "\''${{ needs.files_modified.outputs.installer == 'true' && needs.files_modified.outputs.full == 'true' }}" ]; then
      echo "installer_agrees must run in the full run when the installer was chosen, and only then" >&2
      exit 1
    fi
    # every output a job reads is one files_modified writes
    have=$(yq -o=json -I=0 '.jobs.files_modified.outputs | keys' ${workflow})
    for o in $(grep -o 'needs\.files_modified\.outputs\.[a-z_]*' ${workflow} | sed 's/.*\.//' | sort -u); do
      if ! echo "$have" | grep -q "\"$o\""; then
        echo "a job reads files_modified's $o, which it does not write" >&2
        exit 1
      fi
    done
    # a pull request builds only the systems its change reaches: the steps
    # skip the "none" that stands for no system (never the whole job)
    if [ "$(yq '[.jobs.build_systems.steps[] | select(.if != "failure()") | .if] | .[0]' ${workflow})" != "\''${{ matrix.host != 'none' }}" ]; then
      echo "build_systems' steps must skip the none host" >&2
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
