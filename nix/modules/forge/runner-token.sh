# shellcheck shell=bash
# This box's registration token for one of its runners (KIND: host or
# members), made by the forge's box and sealed to this box's own secrets key
# (nix/modules/forge/runner-keys.nix): fetched, opened here, kept for the
# runner. Run as root by runner-start.sh, once the forge answers, before it
# drops to the runner's user. A runner already registered needs none, so a forge that does not
# answer leaves the last one, or none, and the runner starts all the same.
# Environment: URL (the forge), NAME (this box), KIND, OUT (the env file),
# IDENTITY (this box's secrets key).
set -u
umask 077
w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
if curl -fsS --max-time 30 "$URL/_dd/runner-keys/$NAME.$KIND.age" -o "$w/token.age" \
  && age -d -i "$IDENTITY" -o "$w/token.env" "$w/token.age" \
  && grep -q '^TOKEN=.' "$w/token.env"; then
  install -m 0600 "$w/token.env" "$OUT"
  echo "the $KIND runner's registration token, from the forge"
elif [ -s "$OUT" ]; then
  echo "no new $KIND token from the forge; the last one stands"
else
  # nothing yet: a runner registered before needs none; one that is not
  # registers once the forge has sealed it a token
  printf 'TOKEN=\n' > "$OUT"
  echo "no $KIND token from the forge yet"
fi
exit 0
