# In the initrd: the disk key from its two halves, before any pool is
# imported. The TPM half is unsealed by the TPM only under this box's own
# signed boot (PCR 7). The other half is the unlock Worker's, asked for with
# a request this TPM signs, from home. Waits, asking again, for as long as
# it takes: the network coming up, or a member letting the box in from
# somewhere new.
#
# The enrolment lives on the boot partition (dd/unlock): sig.pub, sig.priv
# and half.pub, half.priv. They open nothing outside this TPM, and files
# put in their place could only make a key that opens nothing.
#
# From the unit: BOX, URL, OUT (a directory).
set -u
umask 077
mkdir -p "$OUT"
w=$(mktemp -d)
KEYS="$w/enrolment"

# the boot partition, as the boot loader says it booted from
esp() {
  local v uuid
  v=/sys/firmware/efi/efivars/LoaderDevicePartUUID-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f
  [ -r "$v" ] || return 1
  uuid=$(tail -c +5 "$v" | tr -d '\0' | tr 'A-Z' 'a-z')
  mkdir -p "$w/esp" "$KEYS"
  mount -o ro "/dev/disk/by-partuuid/$uuid" "$w/esp" || return 1
  cp "$w/esp/dd/unlock/"* "$KEYS"/ 2>/dev/null
  umount "$w/esp"
  [ -e "$KEYS/sig.priv" ] && [ -e "$KEYS/half.priv" ]
}
tries=0
until esp; do
  tries=$((tries + 1))
  if [ "$tries" -ge 30 ]; then
    echo "dd-unlock: no enrolment on the boot partition; the pools stay locked"
    exit 0
  fi
  sleep 1
done

b64url() { base64 -w0 | tr '+/' '-_' | tr -d '='; }

# the router's hardware address, as this box sees it: the second sign of home
router() {
  local gw
  gw=$(ip -4 route show default 2>/dev/null | awk '{print $3; exit}')
  [ -n "$gw" ] || return 0
  ping -c1 -W1 "$gw" >/dev/null 2>&1 || true
  ip -4 neigh show "$gw" 2>/dev/null | awk '/lladdr/ {print tolower($5); exit}'
}

sign() {
  # the parent is made the same each time from the TPM's own seed; the key
  # under it can only be loaded here, and used only under PCR 7 as it was
  tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx" &&
    tpm2_load -Q -C "$w/srk.ctx" -u "$KEYS/sig.pub" -r "$KEYS/sig.priv" -c "$w/sig.ctx" &&
    tpm2_startauthsession -Q --policy-session -S "$w/s.ctx" &&
    tpm2_policypcr -Q -S "$w/s.ctx" -l sha256:7 &&
    tpm2_sign -Q -c "$w/sig.ctx" -g sha256 -s ecdsa -f plain -p "session:$w/s.ctx" -o "$w/sig.bin" "$1"
  local r=$?
  tpm2_flushcontext "$w/s.ctx" >/dev/null 2>&1 || true
  tpm2_flushcontext -t >/dev/null 2>&1 || true
  return $r
}

say() { echo "$*" >/dev/console 2>/dev/null || true; echo "$*"; }

wait=5
share=
while [ -z "$share" ]; do
  mac=$(router)
  at=$(date +%s)
  printf 'commonty unlock v1\0%s\0%s\0%s' "$BOX" "$at" "$mac" >"$w/msg"
  if ! sign "$w/msg"; then
    say "dd-unlock: the TPM would not sign; asking again in ${wait}s"
  else
    sig=$(b64url <"$w/sig.bin")
    code=$(curl -sS --max-time 20 -o "$w/answer" -w '%{http_code}' -X POST "$URL/api/unlock" \
      -H 'content-type: application/json' \
      --data "{\"box\":\"$BOX\",\"at\":$at,\"mac\":\"$mac\",\"sig\":\"$sig\"}" 2>"$w/err") || code=000
    case "$code" in
      200) share=$(jq -r '.share // empty' "$w/answer") ;;
      202) say "dd-unlock: this box started somewhere new; a member can let it in from their boxes page"; wait=30 ;;
      403) say "dd-unlock: refused: $(jq -r '.error // empty' "$w/answer" 2>/dev/null)"; wait=300 ;;
      000) say "dd-unlock: no network yet ($(head -c 120 "$w/err"))" ;;
      *) say "dd-unlock: the unlock service said $code" ;;
    esac
  fi
  if [ -z "$share" ]; then
    sleep "$wait"
    [ "$wait" -lt 30 ] && wait=$((wait * 2))
  fi
done

# The TPM's half, unsealed under PCR 7 as it was at enrolment
tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
tpm2_load -Q -C "$w/srk.ctx" -u "$KEYS/half.pub" -r "$KEYS/half.priv" -c "$w/half.ctx"
tpm2_startauthsession -Q --policy-session -S "$w/h.ctx"
tpm2_policypcr -Q -S "$w/h.ctx" -l sha256:7
tpm2_unseal -c "$w/half.ctx" -p "session:$w/h.ctx" -o "$w/half"
tpm2_flushcontext "$w/h.ctx" >/dev/null 2>&1 || true

# The key: HKDF over both halves, named for this box; the second pool's key
# from it. Neither half alone says anything about it.
ikm=$( { cat "$w/half"; printf '%s' "$share" | tr -- '-_' '+/' | sed 's/$/=/' | base64 -d 2>/dev/null; } | od -An -v -tx1 | tr -d ' \n')
hkdf() {
  openssl kdf -keylen 32 -kdfopt digest:SHA256 -kdfopt "hexkey:$1" \
    -kdfopt "salt:$BOX" -kdfopt "info:$2" -binary HKDF
}
hkdf "$ikm" dd-disk-v1 >"$OUT/disk.key"
hkdf "$ikm" dd-vault-v1 >"$OUT/vault.key"
chmod 0400 "$OUT"/*.key
rm -rf "$w"
say "dd-unlock: unlocked"
