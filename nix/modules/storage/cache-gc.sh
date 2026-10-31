# shellcheck shell=bash
# The nix cache, held to what is still wanted: every path under the systems
# of the last KEEP releases, and anything uploaded in the last AGE days (a
# CI run's builds, a release on its way). Everything else goes. SOURCE is the
# fleet's repository at the public door (its releases branch says which
# systems each release had), whichever box holds the forge; CACHE the cache's
# nix store url; BUCKET and ENDPOINT where its objects are. DRY=1 counts and
# deletes nothing.
set -euo pipefail
REPO=$TMP/repo.git
git clone -q --bare --single-branch --branch releases "$SOURCE" "$REPO" || {
  echo "the forge did not answer: next time" >&2
  exit 0
}
mapfile -t roots < <(git -c safe.directory="$REPO" -C "$REPO" log -n "$KEEP" --format=%H releases -- current.json |
  while read -r c; do git -c safe.directory="$REPO" -C "$REPO" show "$c:current.json" | jq -r '.payload.boxes[].path'; done | sort -u)
[ "${#roots[@]}" -gt 0 ] || { echo "no releases found; nothing removed" >&2; exit 1; }
# every path under them, as the cache has it; a root it does not have stops
# the whole run rather than letting its closure look unwanted
nix path-info --recursive --json --json-format 1 --store "$CACHE" "${roots[@]}" >"$TMP/live.json"
jq -r 'if type == "array" then .[] else to_entries[] | .value + {path: .key} end
  | (.path | sub("^/nix/store/"; "") | .[0:32]) + ".narinfo", .url' "$TMP/live.json" | sort -u >"$TMP/keep"
[ "$(wc -l <"$TMP/keep")" -gt 1000 ] || { echo "only $(wc -l <"$TMP/keep") wanted objects: refusing" >&2; exit 1; }
cutoff=$(date -u -d "-$AGE days" +%Y-%m-%dT%H:%M:%S)
aws --endpoint-url "$ENDPOINT" s3api list-objects-v2 --bucket "$BUCKET" \
  --query 'Contents[].[Key, LastModified]' --output text >"$TMP/all"
# old, and wanted by no kept release
awk -v c="$cutoff" '$2 < c { print $1 }' "$TMP/all" | sort -u | comm -23 - "$TMP/keep" >"$TMP/gone"
echo "$(wc -l <"$TMP/all") objects, $(wc -l <"$TMP/keep") wanted, $(wc -l <"$TMP/gone") to remove"
[ "${DRY:-0}" = 1 ] && exit 0
split -l 1000 "$TMP/gone" "$TMP/batch."
for b in "$TMP"/batch.*; do
  [ -e "$b" ] || continue
  jq -R '{Key: .}' "$b" | jq -s '{Objects: ., Quiet: true}' >"$b.json"
  aws --endpoint-url "$ENDPOINT" s3api delete-objects --bucket "$BUCKET" --delete "file://$b.json" >/dev/null
done
echo "removed $(wc -l <"$TMP/gone")"
