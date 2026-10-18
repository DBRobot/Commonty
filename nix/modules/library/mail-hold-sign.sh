# shellcheck shell=bash
# One signature for the gate's taking of held mail (servers/gate/src/mail.rs):
# the message on the socket, signed by this box's TPM key as its unlock and
# its mail are, answered as base64url. Only a message that is the mail
# Worker's hold request for this box is signed: the gate holds no key of the
# box's, and cannot have this sign anything else.
# From the unit: BOX, KEYS (the TPM key's files), and SOFT_KEY in a test.
set -u
umask 077
w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
head -c 4096 >"$w/msg"
# "commonty mail hold v1\0<box>\0<ms>\0GET|POST\0/api/hold/...\0<sha256 hex>"
want=$(printf 'commonty mail hold v1\0%s\0' "$BOX" | od -An -tx1 | tr -d ' \n')
have=$(head -c $((22 + ${#BOX} + 1)) "$w/msg" | od -An -tx1 | tr -d ' \n')
[ "$have" = "$want" ] || { echo "dd-mail-hold-sign: refused a message that is not a hold request" >&2; exit 0; }
fields=$(tr '\0' '\n' <"$w/msg" | wc -l)
[ "$fields" -eq 5 ] || { echo "dd-mail-hold-sign: refused a malformed hold request" >&2; exit 0; }
path=$(tr '\0' '\n' <"$w/msg" | sed -n 5p)
case "$path" in
  /api/hold/list\?* | /api/hold/get\?* | /api/hold/ack\?* | /api/hold/watch) ;;
  *) echo "dd-mail-hold-sign: refused a path outside the hold" >&2; exit 0 ;;
esac
# shellcheck source=nix/modules/gate/box-sign.sh
. @boxSign@
box_sign "$w/msg" "$w/sig" || { echo "dd-mail-hold-sign: the TPM would not sign" >&2; exit 0; }
base64 -w0 <"$w/sig" | tr '+/' '-_' | tr -d '='
