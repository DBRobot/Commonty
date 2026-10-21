# shellcheck shell=bash
# What is wrong on any box, as the fleet view has it, mailed once per change:
# disk events in the last week, a pool that is not healthy, a backup over a
# day old, a runner's new config pending over an hour, a release's rollout
# that stopped (docs/dev/rollout.md). QUERY is thanos-query's api; STATE
# remembers what was mailed; SENDMAIL is dd-sendmail; WINDOW is how far back
# to look: another box's newest numbers are only in the bucket once its block
# is cut, so an instant query would not see them.
set -eu
# Every box writes its own numbers and its own name into what this reads,
# and no box is trusted: a name is kept only if it is a name, a value only if
# it is a plain number, before either reaches arithmetic or a mail.
ask() {
  curl -fsS -m 30 --get --data-urlencode "query=$1" "$QUERY" |
    jq -r '.data.result[]
      | select((.metric.box // "") | test("^[A-Za-z0-9._-]{1,64}$"))
      | select(.value[1] | test("^[0-9]+(\\.[0-9]+)?$"))
      | "\(.metric.box) \(.value[1] | split(".")[0])"'
}
now=$(date +%s)
lines=$(
  {
    ask "max by (box) (max_over_time(dd_disk_warnings[$WINDOW])) > 0" | while read -r b v; do echo "$b: $v disk warning(s) in the last week (journalctl -u smartd there)"; done
    ask "max by (box) (max_over_time(dd_pool_unhealthy[$WINDOW])) > 0" | while read -r b _; do echo "$b: a ZFS pool is not healthy (zpool status -x there)"; done
    # every box says so of the same agreed stop: one line is enough
    ask "topk(1, max by (box) (max_over_time(dd_rollout_bad[$WINDOW]))) > 0" | while read -r b _; do echo "a release's staged rollout stopped, so no other box takes it (seen by $b; dd release status says why)"; done
    ask "max by (box) (max_over_time(dd_backup_last_success_seconds[$WINDOW])) < $((now - 26 * 3600))" | while read -r b v; do
      echo "$b: last good backup $(((now - v) / 3600)) hours ago"
    done
    # a runner's or docker's new config (a sandbox fix, say) not in force yet
    ask "max by (box) (last_over_time(dd_runner_restart_pending_seconds[$WINDOW])) > 3600" | while read -r b _; do
      echo "$b: a CI runner's or docker's new config has waited over an hour to apply (journalctl -u dd-runner-restart there)"
    done
  } | sort
)
[ -s "$STATE" ] || echo '""' >"$STATE"
before=$(jq -r . "$STATE")
# the same trouble is mailed once; a change (new, or more of it) again. A
# backup's age grows every hour and is the same trouble: compared without it
# (it mailed every hour for days, 2026-10-12 to 17, about one stale backup)
same() { sed -E 's/last good backup [0-9]+ hours ago/last good backup over a day ago/'; }
if [ -n "$lines" ] && [ "$(printf '%s' "$lines" | same)" != "$(printf '%s' "$before" | same)" ]; then
  # not sent: nothing is remembered, and the next run tries again
  if ! printf 'Subject: fleet: something needs a look\n\n%s\n' "$lines" | "$SENDMAIL" "$TO"; then
    echo "not mailed; again next time"
    exit 0
  fi
fi
jq -n --arg l "$lines" '$l' >"$STATE"
