# shellcheck shell=bash
# What is wrong on any box, as the fleet view has it, mailed once per change:
# disk events in the last week, a pool that is not healthy, a backup over a
# day old, a runner's new config pending over an hour, the network reaper's
# brake on, a release's rollout that stopped (docs/dev/rollout.md), a
# validator that restarted itself over a wedged consensus, a box off its
# wired link (and, once, when it is back on it). QUERY is thanos-query's api;
# STATE remembers what was mailed; WIRED the boxes mailed as off their wired
# link; SENDMAIL is dd-sendmail; WINDOW is how far back to look: another
# box's newest numbers are only in the bucket once its block is cut, so an
# instant query would not see them.
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
# off the cable for over five minutes (box/wired-watch.sh), and back on it
off=$(ask "(max by (box) (last_over_time(dd_wired_since_seconds[$WINDOW])) < $((now - 300))) and on (box) (max by (box) (last_over_time(dd_wired_state[$WINDOW])) > 0)")
back=$(ask "max by (box) (last_over_time(dd_wired_state[$WINDOW])) == 0" | cut -d' ' -f1)
lines=$(
  {
    ask "max by (box) (max_over_time(dd_disk_warnings[$WINDOW])) > 0" | while read -r b v; do echo "$b: $v disk warning(s) in the last week (journalctl -u smartd there)"; done
    ask "max by (box) (max_over_time(dd_pool_unhealthy[$WINDOW])) > 0" | while read -r b _; do echo "$b: a ZFS pool is not healthy (zpool status -x there)"; done
    ask "max by (box) (last_over_time(dd_reap_brake[$WINDOW])) > 0" | while read -r b _; do echo "$b: the network reaper refuses a state that looks wrong and has taken nothing off (journalctl -u dd-verify there: 'the brake is on')"; done
    # every box says so of the same agreed stop: one line is enough
    ask "topk(1, max by (box) (max_over_time(dd_rollout_bad[$WINDOW]))) > 0" | while read -r b _; do echo "a release's staged rollout stopped, so no other box takes it (seen by $b; dd release status says why)"; done
    # a validator that found its consensus wedged and restarted itself in
    # the last day: once per restart, by when (docs/dev/control-plane.md)
    ask "max by (box) (max_over_time(dd_control_selfheal_last_seconds[$WINDOW])) > $((now - 24 * 3600))" | while read -r b v; do
      echo "$b: dd-control found its consensus wedged and restarted itself at $(date -u -d "@$v" '+%F %H:%M') UTC (journalctl -u dd-control there: 'self-heal')"
    done
    ask "max by (box) (max_over_time(dd_backup_last_success_seconds[$WINDOW])) < $((now - 26 * 3600))" | while read -r b v; do
      echo "$b: last good backup $(((now - v) / 3600)) hours ago"
    done
    # a remote computer's backup held back, to spare a store's last room
    ask "max by (box) (last_over_time(dd_store_copy_skipped[$WINDOW])) > 0" | while read -r b _; do
      echo "$b: a remote computer's backup was left uncopied: a store is short of room (journalctl -u dd-store-copy)"
    done
    # a runner's or docker's new config (a sandbox fix, say) not in force yet
    ask "max by (box) (last_over_time(dd_runner_restart_pending_seconds[$WINDOW])) > 3600" | while read -r b _; do
      echo "$b: a CI runner's or docker's new config has waited over an hour to apply (journalctl -u dd-runner-restart there)"
    done
    while read -r b v; do
      [ -n "$b" ] && echo "$b: on Wi-Fi since $(date -d "@$v" '+%a %d %b %H:%M'): the cable to the router stopped passing traffic and the box could not bring it back; the cable and its adapter need a look"
    done <<<"$off"
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

# a box mailed as off its wired link that is on it again: said once
[ -s "$WIRED" ] || echo '[]' >"$WIRED"
names() { jq -R 'select(. != "")' | jq -s .; }
was=$(cat "$WIRED")
off=$(cut -s -d' ' -f1 <<<"$off" | names)
back=$(names <<<"$back")
again=$(jq -rn --argjson was "$was" --argjson back "$back" '$was - ($was - $back) | .[] | "\(.): back on its wired link"')
if [ -n "$again" ] && ! printf 'Subject: fleet: back to normal\n\n%s\n' "$again" | "$SENDMAIL" "$TO"; then
  echo "not mailed; again next time"
  exit 0
fi
# still off, or not yet in the view either way
jq -n --argjson was "$was" --argjson back "$back" --argjson off "$off" '$was - $back + $off | unique' >"$WIRED.tmp"
mv -f "$WIRED.tmp" "$WIRED"
