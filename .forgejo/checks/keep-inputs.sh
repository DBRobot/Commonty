#!/usr/bin/env bash
# keep-inputs.sh <key> <file>...: keep what this job's builds were built
# from through the box's GC. The files list what the job built (nix build
# --print-out-paths), so nothing is evaluated again. Every output of the
# build graph under those that this box built itself is rooted under
# ~/ci-keep/<key>, in place of what this key kept before; so after the
# weekly GC the next run finds the cross rustc, crane's dependency builds
# and the crates under the tests and clippy, and builds only what changed.
# A key no job has refreshed in a week is let go. It never holds more than
# KEEP_PERCENT of the store's filesystem (the box's GC needs room to make
# min-free), and never fails the job. DRY=1 only says what it would keep.
set -u
key=$1
shift
keep=$HOME/ci-keep
percent=${KEEP_PERCENT:-10}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

find "$keep" -mindepth 1 -maxdepth 1 -type d -mtime +7 -exec rm -rf {} + 2>/dev/null

# what the job built, by the derivations they came from
cat "$@" 2>/dev/null | grep '^/nix/store/' | sort -u | xargs -r nix-store --query --deriver 2>/dev/null \
  | grep '\.drv$' | sort -u | xargs -r nix-store --check-validity --print-invalid > "$tmp/nodrv"
cat "$@" 2>/dev/null | grep '^/nix/store/' | sort -u | xargs -r nix-store --query --deriver 2>/dev/null \
  | grep '\.drv$' | sort -u | comm -23 - "$tmp/nodrv" > "$tmp/top"
[ -s "$tmp/top" ] || { echo "keep-inputs $key: nothing built to keep"; exit 0; }
# every derivation the builds went through, and its outputs this box has
xargs nix-store --query --requisites < "$tmp/top" | grep '\.drv$' > "$tmp/drvs"
xargs nix-store --query --outputs < "$tmp/drvs" | sort -u > "$tmp/outs"
xargs nix-store --check-validity --print-invalid < "$tmp/outs" | sort -u > "$tmp/gone"
# only what this box built itself: what it downloaded it downloads again
# in seconds after a GC, and a host's runtime closure is held already
comm -23 "$tmp/outs" "$tmp/gone" > "$tmp/here"
# shellcheck disable=SC2046
nix path-info --json --json-format 1 $(cat "$tmp/here") | node -e '
  let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
    const j = JSON.parse(s);
    const e = Array.isArray(j) ? j.map((x) => [x.path, x]) : Object.entries(j);
    for (const [p, x] of e) if (x && x.ultimate) console.log(p);
  });' | sort -u > "$tmp/have"
size() { [ -s "$1" ] && xargs nix path-info --size < "$1" 2>/dev/null | awk '{s += $2} END {print s + 0}' || echo 0; }
bytes=$(size "$tmp/have")
# the cap is on everything kept, every key's, each path once
cp "$tmp/have" "$tmp/total"
for d in "$keep"/*/; do
  case $(basename "$d") in "$key".*) ;; *) cat "$d/list" 2>/dev/null ;; esac
done >> "$tmp/total"
sort -u -o "$tmp/total" "$tmp/total"
total=$(size "$tmp/total")
fs=$(df --output=size -B1 /nix/store | tail -1)
cap=$((fs * percent / 100))
gib() { awk -v b="$1" 'BEGIN {printf "%.1fG", b / 2^30}'; }
echo "keep-inputs $key: $(wc -l < "$tmp/have") built here of $(wc -l < "$tmp/outs") outputs, $(gib "$bytes"); all keys $(gib "$total") of at most $(gib "$cap") ($percent% of $(gib "$fs"))"
[ "${DRY:-}" = 1 ] && { cp "$tmp/have" "${DRY_OUT:-/dev/null}"; exit 0; }

mkdir -p "$keep"
new=$(mktemp -d -p "$keep" "$key.XXXXXX")
if [ "$total" -gt "$cap" ]; then
  echo "keep-inputs $key: over the cap, keeping nothing for it"
elif [ -s "$tmp/have" ]; then
  # an out-link a path (r, r-1, ...), each an indirect root: one call, so
  # the numbering never starts over (1200 in 4s on node1)
  # shellcheck disable=SC2046
  nix build --option substitute false --out-link "$new/r" $(cat "$tmp/have") >/dev/null \
    || { echo "keep-inputs $key: rooting failed, keeping the last set"; rm -rf "$new"; exit 0; }
  cp "$tmp/have" "$new/list"
fi
for old in "$keep/$key".*; do
  [ "$old" = "$new" ] || rm -rf "$old"
done
exit 0
