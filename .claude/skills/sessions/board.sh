#!/usr/bin/env bash
# The sessions' shared board on this machine (SKILL.md beside this file):
#   board.sh show
#   board.sh set <me> "<line>" [paths...]   what you are doing, and the paths you claim
#   board.sh drop <me>
#   board.sh queue <me>                     join the line for the release lock
#   board.sh lock <me>                      wait to be first with the lock free, then hold it until unlock
#   board.sh unlock <me>
set -euo pipefail

dir=~/.cache/sessions
board=$dir/board
queue=$dir/queue
lockfile=~/.cache/dd-release.lock
mkdir -p "$board"
touch "$queue" "$lockfile"

name_ok() { [[ "${1:-}" =~ ^[A-Za-z0-9._()\ -]{1,80}$ ]] || { echo "board: give your session's name" >&2; exit 2; }; }
slug() { printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_'; }
# the queue is edited under its own lock, never the release lock
with_queue() { ( flock 8; "$@" ) 8>"$queue.lock"; }
queue_add() { grep -qxF -- "$1" "$queue" || printf '%s\n' "$1" >>"$queue"; }
queue_del() {
  local t
  t=$(mktemp -p "$dir")
  grep -vxF -- "$1" "$queue" >"$t" || true
  mv "$t" "$queue"
}

holder() {
  if flock -n "$lockfile" true 2>/dev/null; then echo "nobody"; else cat "$dir/holder" 2>/dev/null || echo "someone not using this board"; fi
}

case "${1:-show}" in
show)
  echo "release lock: $(holder)"
  echo "queue:"
  if [ -s "$queue" ]; then nl -w2 -s'. ' "$queue"; else echo "  (empty)"; fi
  echo "sessions:"
  shopt -s nullglob
  files=("$board"/*)
  [ ${#files[@]} -gt 0 ] || echo "  (none)"
  for f in "${files[@]}"; do
    sed 's/^/  /' "$f"
    echo
  done
  ;;
set)
  name_ok "${2:-}"
  me=$2 line=${3:?"board: say what you are doing"}
  shift 3
  {
    echo "$me ($(date '+%F %H:%M')): $line"
    for p in "$@"; do echo "  claims $p"; done
  } >"$board/$(slug "$me")"
  ;;
drop)
  name_ok "${2:-}"
  rm -f -- "$board/$(slug "$2")"
  ;;
queue)
  name_ok "${2:-}"
  with_queue queue_add "$2"
  echo "in line: $(grep -nxF -- "$2" "$queue" | cut -d: -f1) of $(wc -l <"$queue")"
  ;;
lock)
  name_ok "${2:-}"
  me=$2
  with_queue queue_add "$me"
  until [ "$(head -n1 "$queue")" = "$me" ]; do sleep 15; done
  exec 9>"$lockfile"
  flock 9
  echo "$me since $(date '+%F %H:%M')" >"$dir/holder"
  echo "lock held by $me"
  # held until unlock removes the marker
  while [ -e "$dir/holder" ] && grep -q "^$me since" "$dir/holder"; do sleep 5; done
  ;;
unlock)
  name_ok "${2:-}"
  if grep -q "^$2 since" "$dir/holder" 2>/dev/null; then rm -f "$dir/holder"; fi
  with_queue queue_del "$2"
  echo "next: $(head -n1 "$queue" 2>/dev/null || true)"
  ;;
*)
  sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
  ;;
esac
