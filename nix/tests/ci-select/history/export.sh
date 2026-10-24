#!/usr/bin/env bash
# The forge's record of every CI run, read only: its runs, jobs, tasks,
# runners, pull requests, each run's base, and every job's log. Run from a
# machine that can ssh to the forge's box as admin.
#
#   export.sh DIR [admin@100.64.0.2]
set -euo pipefail
dir=$1 box=${2:-admin@100.64.0.2}
mkdir -p "$dir/L"
q() { ssh "$box" 'sudo -u forgejo psql -d forgejo -q' <<<"\\copy ($1) to stdout with (format csv, header)" >"$dir/$2"; }
repo="(select repo_id from action_run where workflow_id = 'entry_point.yml' order by id desc limit 1)"
q "select r.id, r.index, r.pull_request_id, r.commit_sha, r.event, r.ref, r.workflow_id, r.status, r.started, r.stopped, r.created, r.previous_duration, translate(r.title, E'\\t\\n', '  ') from action_run r where r.repo_id = $repo" runs.csv
q "select j.id, j.run_id, j.name, j.job_id, j.attempt, j.status, j.started, j.stopped, j.task_id from action_run_job j where j.repo_id = $repo" jobs.csv
q "select id, job_id, attempt, runner_id, status, started, stopped, log_filename from action_task where repo_id = $repo" tasks.csv
q "select id, name from action_runner" runners.csv
q "select id, event_payload::json->'pull_request'->'base'->>'sha' as base, event_payload::json->>'action' as action from action_run where repo_id = $repo" base.csv
q "select p.id, i.index, p.head_branch, p.merged_commit_id, p.merge_base, p.has_merged, p.merged_unix, i.is_closed from pull_request p join issue i on i.id = p.issue_id where i.repo_id = $repo" prs.csv
for owner_repo in $(ssh "$box" 'sudo sh -c "cd /vault/forgejo/data/actions_log && ls -d */*"'); do
  mkdir -p "$dir/L/$owner_repo"
  # shellcheck disable=SC2029 # the path is meant to expand here
  ssh "$box" "sudo tar -C /vault/forgejo/data/actions_log/$owner_repo -cf - ." | tar -x -C "$dir/L/$owner_repo"
done
