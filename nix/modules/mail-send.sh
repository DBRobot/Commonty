# shellcheck shell=bash
# One mail from this box, for dd-sendmail on the other end of the socket:
# a line of recipients, then the message. Signed by this box's TPM key, as
# its unlock requests are, and handed to the mail Worker (client/mail/worker/
# send.js), which sends it on to members and the owner and to nobody else.
# Answers one line, "ok" or "error: ...". Never logs the message.
# From the unit: BOX, URL, DOMAIN, KEYS (the TPM key's files), MAX (bytes).
set -u
umask 077
w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT

IFS= read -r line || { echo "error: nothing sent"; exit 0; }
to=
set -f
for r in $line; do
  r=${r,,}
  if [ "$r" != alerts ] && ! { [[ "$r" =~ ^[a-z0-9][a-z0-9._-]{0,63}@(.+)$ ]] && [ "${BASH_REMATCH[1]}" = "$DOMAIN" ]; }; then
    echo "error: $r is neither alerts nor a member's @$DOMAIN"
    exit 0
  fi
  case ",$to," in *",$r,"*) continue ;; esac
  to=${to:+$to,}$r
done
[ -n "$to" ] || { echo "error: no recipient"; exit 0; }

head -c $((MAX + 1)) >"$w/msg"
if [ "$(stat -c %s "$w/msg")" -gt "$MAX" ]; then
  echo "error: over $MAX bytes"
  exit 0
fi

at=$(date +%s)
nonce=$(head -c 18 /dev/urandom | base64 -w0 | tr '+/' '-_')
digest=$(sha256sum "$w/msg" | cut -c1-64)
printf 'commonty mail send v1\0%s\0%s\0%s\0%s\0%s' "$BOX" "$at" "$nonce" "$to" "$digest" >"$w/sign"

# the same key and policy as the unlock (box/disk-unlock-boot.sh); one
# user of the TPM at a time on the box (cmd/dd-control/tpmsign.go)
lock=/run/dd-tpm/lock
# root's alone (box/control.nix); without it, a lock of its own
if [ ! -r "$lock" ]; then
  lock="$w/lock"
  : >"$lock"
fi
{
  flock -w 10 9 || true
  tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx" &&
    tpm2_load -Q -C "$w/srk.ctx" -u "$KEYS/sig.pub" -r "$KEYS/sig.priv" -c "$w/sig.ctx" &&
    tpm2_startauthsession -Q --policy-session -S "$w/s.ctx" &&
    dd-tpm-policy "$w/s.ctx" "$KEYS" &&
    tpm2_sign -Q -c "$w/sig.ctx" -g sha256 -s ecdsa -f plain -p "session:$w/s.ctx" -o "$w/sig.bin" "$w/sign"
  r=$?
  tpm2_flushcontext "$w/s.ctx" >/dev/null 2>&1 || true
  tpm2_flushcontext -t >/dev/null 2>&1 || true
} 9<"$lock"
if [ "$r" != 0 ]; then
  echo "dd-mail: the TPM would not sign" >&2
  echo "error: the TPM would not sign"
  exit 0
fi
sig=$(base64 -w0 <"$w/sig.bin" | tr '+/' '-_' | tr -d '=')

code=$(curl -sS --max-time 30 -o "$w/answer" -w '%{http_code}' -X POST "$URL/send" \
  -H "x-dd-box: $BOX" -H "x-dd-at: $at" -H "x-dd-nonce: $nonce" \
  -H "x-dd-to: $to" -H "x-dd-sig: $sig" \
  -H 'content-type: message/rfc822' --data-binary @"$w/msg" 2>"$w/err") || code=000
if [ "$code" = 200 ]; then
  echo "dd-mail: sent to $to" >&2
  echo ok
else
  why=$(jq -r '.error // empty' "$w/answer" 2>/dev/null | head -c 200)
  [ "$code" = 000 ] && why=$(head -c 200 "$w/err")
  echo "dd-mail: not sent to $to: $code $why" >&2
  echo "error: $code $why"
fi
