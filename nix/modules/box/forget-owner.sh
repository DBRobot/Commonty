# shellcheck shell=bash
# A joined box whose owner it no longer serves is a stranger, which keeps only
# ciphertext copies: every remote computer's disk on it and every game server
# and kept world, its former owner's and those of any member who ran one here,
# go. Started only by dd-control (cmd/dd-control/enlisted.go), on the plan's
# word and its own reading of the agreed state, naming the owner it knew. Safe
# to run again. What it could not remove yet (still running) keeps it failing, and
# dd-control tries again: only when all is gone does the member come off what
# this box has known, so a member it never knew can never be asked for.
set -u
root=${FORGET_ROOT:-}
member=${1:-}
[[ $member =~ ^[a-z0-9][a-z0-9_-]{0,31}$ ]] || {
  echo "not a member's name" >&2
  exit 2
}
seen="$root/var/lib/dd-control/owners-seen/$member"
[ -e "$seen" ] || {
  echo "this box never knew them as its owner: nothing done" >&2
  exit 2
}
left=0

owners="$root/var/lib/dd-computer-owners"
for f in "$owners"/*; do
  [ -f "$f" ] || continue
  id=$(basename "$f")
  [[ $id =~ ^[0-9a-f]{16}$ ]] || continue
  if [ -e "$root/run/dd-placed/computer-$id" ]; then
    echo "computer-$id still runs here" >&2
    left=1
    continue
  fi
  rm -rf -- "$root/var/lib/private/dd-computer/$id" "$root/var/lib/dd-computer/$id"
  rm -f -- "$f"
  echo "computer-$id: removed"
done

games="$root/var/lib/dd-games"
if [ -d "$games/records" ]; then
  if [ -e "$root/run/dd-placed/dd-games" ]; then
    echo "the games still run here" >&2
    left=1
  else
    for r in "$games"/records/*.json; do
      [ -f "$r" ] || continue
      id=$(basename "$r" .json)
      [[ $id =~ ^[A-Za-z0-9_-]+$ ]] || continue
      rm -rf -- "$games/instances/$id"
      rm -f -- "$games/players/$id.json" "$games/control/$id.qmp" "$r"
      echo "game server $id: removed"
    done
    rm -rf -- "${games:?}/worlds"
  fi
fi

[ "$left" = 0 ] || exit 1
rm -f -- "$seen"
echo "nothing of theirs is left here"
