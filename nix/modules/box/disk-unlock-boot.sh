# shellcheck shell=bash
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
# From the unit: BOX, URL, OUT (a directory), and how long to wait:
# ENROL_WAIT (s, for the boot partition), WAIT_WAITING (s, while a member
# has not let it in), WAIT_REFUSED (s, while it is marked stolen), TPM_ONLY
# (set: a box that joined on its own, whose key is the TPM's half alone).
set -u
umask 077
mkdir -p "$OUT"
w=$(mktemp -d)
KEYS="$w/enrolment"

# the boot partition, as the boot loader says it booted from
part=
v=/sys/firmware/efi/efivars/LoaderDevicePartUUID-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f
if [ -r "$v" ]; then
  part="/dev/disk/by-partuuid/$(tail -c +5 "$v" | tr -d '\0' | tr '[:upper:]' '[:lower:]')"
fi
esp() {
  # on the running box it is mounted already
  if [ -e /boot/dd/unlock/sig.priv ]; then
    mkdir -p "$KEYS" && cp -r /boot/dd/unlock/* "$KEYS"/
    return
  fi
  [ -n "$part" ] || return 1
  mkdir -p "$w/esp" "$KEYS"
  mount -o ro "$part" "$w/esp" || return 1
  cp -r "$w/esp/dd/unlock/"* "$KEYS"/ 2>/dev/null
  umount "$w/esp"
  # on its TPM alone, no signing key is asked for (a joined box makes its
  # own at its first boot); otherwise it signs its ask to the Worker
  if [ -n "$TPM_ONLY" ]; then
    [ -e "$KEYS/half.priv" ] || [ -e "$KEYS/bootstrap.priv" ]
  else
    [ -e "$KEYS/sig.priv" ] && [ -e "$KEYS/half.priv" ]
  fi
}
# The firmware booted from that partition, so its disk is there: udev naming
# it is only slow (on a busy host, past ENROL_WAIT), and is not "no enrolment".
# 90s, as systemd waits for a device.
if [ -n "$part" ] && [ ! -e /boot/dd/unlock/sig.priv ]; then
  t=0
  while [ ! -e "$part" ] && [ "$t" -lt 90 ]; do
    sleep 1
    t=$((t + 1))
  done
  [ -e "$part" ] || echo "dd-unlock: the boot partition $part did not appear"
fi
tries=0
until esp; do
  tries=$((tries + 1))
  if [ "$tries" -ge "$ENROL_WAIT" ]; then
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
    dd-tpm-policy "$w/s.ctx" "$KEYS" &&
    tpm2_sign -Q -c "$w/sig.ctx" -g sha256 -s ecdsa -f plain -p "session:$w/s.ctx" -o "$w/sig.bin" "$1"
  local r=$?
  tpm2_flushcontext "$w/s.ctx" >/dev/null 2>&1 || true
  tpm2_flushcontext -t >/dev/null 2>&1 || true
  return $r
}

say() { echo "$*" >/dev/console 2>/dev/null || true; echo "$*"; }

wait=2
share=
[ -z "$TPM_ONLY" ] || share=tpm-only
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
      202) say "dd-unlock: this box started somewhere new; a member can let it in from their boxes page"; wait=$WAIT_WAITING ;;
      403) say "dd-unlock: refused: $(jq -r '.error // empty' "$w/answer" 2>/dev/null)"; wait=$WAIT_REFUSED ;;
      # nothing kept for this box: asking again changes nothing. Its pools
      # stay locked; the paper key opens them (dd disk recover)
      404) say "dd-unlock: the unlock service keeps no half for $BOX; the pools stay locked"; exit 0 ;;
      # no answer at all: back off, up to half a minute
      000) say "dd-unlock: no network yet ($(head -c 120 "$w/err"))"; wait=$((wait < 15 ? wait * 2 : 30)) ;;
      *) say "dd-unlock: the unlock service said $code"; wait=$((wait < 15 ? wait * 2 : 30)) ;;
    esac
  fi
  if [ -z "$share" ]; then
    sleep "$wait"
  fi
done

# The TPM's half, unsealed under PCR 7 as it was at enrolment
tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
# a first-boot seal stopped after its re-key: its new half is the one
if [ -n "$TPM_ONLY" ] && [ -e "$KEYS/rekeyed" ] && [ -e "$KEYS/half.priv.new" ]; then
  mv "$KEYS/half.pub.new" "$KEYS/half.pub" && mv "$KEYS/half.priv.new" "$KEYS/half.priv"
fi
if [ -e "$KEYS/half.priv" ]; then
  tpm2_load -Q -C "$w/srk.ctx" -u "$KEYS/half.pub" -r "$KEYS/half.priv" -c "$w/half.ctx"
  tpm2_startauthsession -Q --policy-session -S "$w/h.ctx"
  dd-tpm-policy "$w/h.ctx" "$KEYS"
  tpm2_unseal -c "$w/half.ctx" -p "session:$w/h.ctx" -o "$w/half" || true
  tpm2_flushcontext "$w/h.ctx" >/dev/null 2>&1 || true
else
  # a box that joined, at its first boot: the installer's half, under no
  # PCR, opens the disk this once; dd-unlock-seal then seals a new half
  # under this signed boot and re-keys the pool, and this one opens nothing
  say "dd-unlock: first boot: the installer's key, once"
  tpm2_load -Q -C "$w/srk.ctx" -u "$KEYS/bootstrap.pub" -r "$KEYS/bootstrap.priv" -c "$w/half.ctx"
  tpm2_unseal -c "$w/half.ctx" -o "$w/half" || true
fi
# no half, no key: never one made from the rest alone. A boot the TPM does
# not know (another system, a changed command line) stops here, locked
if [ "$(wc -c <"$w/half" 2>/dev/null || echo 0)" -ne 32 ]; then
  say "dd-unlock: the TPM would not open its half under this boot; the pools stay locked"
  rm -rf "$w"
  exit 1
fi

# The key: HKDF over both halves, named for this box; the second pool's key
# from it. Neither half alone says anything about it. A box on its TPM
# alone: over the half, under one name (join-install.sh makes the same)
salt=$BOX
if [ -n "$TPM_ONLY" ]; then
  ikm=$(od -An -v -tx1 <"$w/half" | tr -d ' \n')
  salt=dd-tpm-only
else
  ikm=$( { cat "$w/half"; printf '%s' "$share" | tr -- '-_' '+/' | sed 's/$/=/' | base64 -d 2>/dev/null; } | od -An -v -tx1 | tr -d ' \n')
fi
hkdf() {
  openssl kdf -keylen 32 -kdfopt digest:SHA256 -kdfopt "hexkey:$1" \
    -kdfopt "salt:$salt" -kdfopt "info:$2" -binary HKDF
}
hkdf "$ikm" dd-disk-v1 >"$OUT/disk.key"
hkdf "$ikm" dd-vault-v1 >"$OUT/vault.key"
chmod 0400 "$OUT"/*.key
rm -rf "$w"
say "dd-unlock: unlocked"
