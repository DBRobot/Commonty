# shellcheck shell=bash
# The runner's main process: the forge, then the registration, then the
# runner. All of it here, none of it before: a switch waits on everything
# before a service's main process, and across every restart, so a forge on
# another box that was down held node2's switch to release 358 and then
# failed both runners, and the release was rolled back for it. Here a
# forge that is down is waited out, however long, with the unit running.
set -eu
dir="$STATE_DIRECTORY/$NAME"
mkdir -p "$dir"
cd "$dir"
wait=5
until curl -fsS -o /dev/null --max-time 20 "$URL/api/v1/version"; do
  echo "the forge at $URL does not answer; again in ${wait}s"
  sleep "$wait"
  wait=$((wait < 300 ? wait * 2 : 300))
done
# registered at another address (the forge renamed): again, at this one
if [ -e .runner ] && ! grep -q "\"address\": \"$URL\"" .runner; then
  echo "runner registered at another address; registering again at $URL"
  rm -f .runner
fi
# as the nixos module registers: again when the token or the labels change,
# in the files it keeps, so a box switching to this finds itself registered
labels=$(printf '%s\n' "$LABELS" | tr ',' '\n' | sort)
hash=$(printf '%s' "$TOKEN" | sha256sum | cut -d' ' -f1)
if [ ! -e .runner ] || [ "$labels" != "$(cat .labels 2>/dev/null || true)" ] ||
  [ "$hash" != "$(cat .token-hash 2>/dev/null || true)" ]; then
  rm -f .runner
  "$RUNNER" register --no-interactive --instance "$URL" --token "$TOKEN" \
    --name "$NAME" --labels "$LABELS" --config "$CONFIG"
  printf '%s' "$hash" >.token-hash
  echo "$labels" >.labels
fi
exec "$RUNNER" daemon --config "$CONFIG"
