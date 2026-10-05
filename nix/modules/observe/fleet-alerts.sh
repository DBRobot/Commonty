# shellcheck shell=bash
# What is wrong on any box, as the fleet view has it, mailed once per change:
# disk events in the last week, a pool that is not healthy, a backup over a
# day old. QUERY is thanos-query's api; STATE remembers what was mailed;
# WINDOW is how far back to look: another box's newest numbers are only in
# the bucket once its block is cut, so an instant query would not see them.
set -eu
ask() {
  curl -fsS -m 30 --get --data-urlencode "query=$1" "$QUERY" |
    jq -r '.data.result[] | "\(.metric.box) \(.value[1])"'
}
now=$(date +%s)
lines=$(
  {
    ask "max by (box) (max_over_time(dd_disk_warnings[$WINDOW])) > 0" | while read -r b v; do echo "$b: $v disk warning(s) in the last week (journalctl -u smartd there)"; done
    ask "max by (box) (max_over_time(dd_pool_unhealthy[$WINDOW])) > 0" | while read -r b _; do echo "$b: a ZFS pool is not healthy (zpool status -x there)"; done
    ask "max by (box) (max_over_time(dd_backup_last_success_seconds[$WINDOW])) < $((now - 26 * 3600))" | while read -r b v; do
      echo "$b: last good backup $(( (now - ${v%.*}) / 3600 )) hours ago"
    done
  } | sort
)
[ -s "$STATE" ] || echo '""' >"$STATE"
before=$(jq -r . "$STATE")
# the same trouble is mailed once; a change (new, or more of it) again
if [ -n "$lines" ] && [ "$lines" != "$before" ]; then
  printf 'Subject: fleet: something needs a look\n\n%s\n' "$lines" | msmtp -a default "$TO"
fi
jq -n --arg l "$lines" '$l' >"$STATE"
