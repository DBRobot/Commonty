# ci-resume.py's choices against a made-up forge that answers as forgejo
# does (its runs, its jobs, an attempt's log, a job's rerun with every job
# that needs it): a job a restart killed runs again by itself and ci_ok
# after it, and nothing that failed for real, was superseded, ran out of
# attempts or is not this repository's CI is ever run again. ci_ok's own
# script, from the workflow, then passes on the rerun's results.
{ pkgs, ... }:
pkgs.runCommand "ci-resume"
  {
    nativeBuildInputs = [
      pkgs.python3
      pkgs.nodejs
      pkgs.yq-go
    ];
  }
  ''
    yq '.jobs.ci_ok.steps[0].run' ${../../.forgejo/workflows/entry_point.yml} > ci_ok.sh
    CI_OK=$PWD/ci_ok.sh python3 ${./ci-resume/test.py} ${../modules/forge/ci-resume.py}
    touch $out
  ''
