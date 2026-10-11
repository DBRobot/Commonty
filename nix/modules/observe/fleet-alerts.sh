# shellcheck shell=bash
# What is wrong on any box, as the fleet view has it, mailed once per change:
# disk events in the last week, a pool that is not healthy, a backup over a
# day old, a box off its wired link (and, once, when it is back on it).
# QUERY is thanos-query's api; STATE remembers what was mailed; WIRED the
# boxes mailed as off their wired link; SENDMAIL is dd-sendmail; WINDOW is
# how far back to look: another box's newest numbers are only in the bucket
# once its block is cut, so an instant query would not see them.
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
    ask "max by (box) (max_over_time(dd_backup_last_success_seconds[$WINDOW])) < $((now - 26 * 3600))" | while read -r b v; do
      echo "$b: last good backup $(((now - v) / 3600)) hours ago"
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
