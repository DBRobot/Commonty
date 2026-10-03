# Once per box, as root, after Secure Boot is on (PCR 7 is what the TPM
# will hold this to):
#   dd-unlock-enrol keys    the TPM's signing key and its half of the disk key,
#                           on the boot partition ($DIR); prints the signing
#                           key for fleet/boxes.json
#   dd-unlock-enrol share   (once the Worker knows that key) hands the Worker
#                           its half, writes the disk keys to /run/dd, and a
#                           copy of each for the owner's paper key in /boot/dd
# The files in $DIR open nothing anywhere but in this TPM, which is why
# they can sit on the unencrypted boot partition for the initrd to read.
#
# From the unit: BOX, URL, DIR, RECOVERY (an ssh-ed25519 public key line).
set -euo pipefail
umask 077
export TPM2TOOLS_TCTI=device:/dev/tpmrm0
mkdir -p "$DIR"
w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
b64url() { base64 -w0 | tr '+/' '-_' | tr -d '='; }

case "${1:-}" in
  keys)
    [ -e "$DIR/sig.pub" ] && { echo "already enrolled: $DIR/sig.pub"; exit 1; }
    tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
    tpm2_startauthsession -Q -S "$w/s.ctx"
    tpm2_policypcr -Q -S "$w/s.ctx" -l sha256:7 -L "$w/pcr.pol"
    tpm2_flushcontext "$w/s.ctx"
    tpm2_create -Q -C "$w/srk.ctx" -G ecc256:ecdsa-sha256 -L "$w/pcr.pol" \
      -a 'fixedtpm|fixedparent|sensitivedataorigin|sign|noda' \
      -u "$DIR/sig.pub" -r "$DIR/sig.priv"
    tpm2_load -Q -C "$w/srk.ctx" -u "$DIR/sig.pub" -r "$DIR/sig.priv" -c "$w/sig.ctx"
    tpm2_readpublic -Q -c "$w/sig.ctx" -f der -o "$w/spki.der"
    tpm2_flushcontext -t >/dev/null 2>&1 || true
    # the TPM's half: random, sealed in this TPM under PCR 7, never written plain
    head -c 32 /dev/urandom >"$w/half"
    tpm2_create -Q -C "$w/srk.ctx" -L "$w/pcr.pol" -a 'fixedtpm|fixedparent|noda' \
      -i "$w/half" -u "$DIR/half.pub" -r "$DIR/half.priv"
    shred -u "$w/half"
    echo "unlockKey for fleet/boxes.json ($BOX):"
    base64 -w0 <"$w/spki.der"
    echo
    ;;
  share)
    [ -e "$DIR/half.priv" ] || { echo "run 'keys' first"; exit 1; }
    head -c 32 /dev/urandom | b64url >"$w/share"
    share=$(cat "$w/share")
    at=$(date +%s)
    printf 'commonty unlock share v1\0%s\0%s\0%s' "$BOX" "$at" "$share" >"$w/msg"
    tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
    tpm2_load -Q -C "$w/srk.ctx" -u "$DIR/sig.pub" -r "$DIR/sig.priv" -c "$w/sig.ctx"
    tpm2_startauthsession -Q --policy-session -S "$w/s.ctx"
    tpm2_policypcr -Q -S "$w/s.ctx" -l sha256:7
    tpm2_sign -Q -c "$w/sig.ctx" -g sha256 -s ecdsa -f plain -p "session:$w/s.ctx" -o "$w/sig.bin" "$w/msg"
    tpm2_flushcontext "$w/s.ctx" >/dev/null 2>&1 || true
    sig=$(b64url <"$w/sig.bin")
    code=$(curl -sS --max-time 20 -o "$w/answer" -w '%{http_code}' -X POST "$URL/api/unlock/share" \
      -H 'content-type: application/json' \
      --data "{\"box\":\"$BOX\",\"at\":$at,\"share\":\"$share\",\"sig\":\"$sig\"}")
    [ "$code" = 200 ] || { echo "the unlock service said $code: $(cat "$w/answer")"; exit 1; }
    # the keys, now, the way the initrd will make them
    tpm2_flushcontext -t >/dev/null 2>&1 || true
    tpm2_load -Q -C "$w/srk.ctx" -u "$DIR/half.pub" -r "$DIR/half.priv" -c "$w/half.ctx"
    tpm2_startauthsession -Q --policy-session -S "$w/h.ctx"
    tpm2_policypcr -Q -S "$w/h.ctx" -l sha256:7
    tpm2_unseal -c "$w/half.ctx" -p "session:$w/h.ctx" -o "$w/half"
    tpm2_flushcontext "$w/h.ctx" >/dev/null 2>&1 || true
    ikm=$( { cat "$w/half"; printf '%s=' "$share" | tr -- '-_' '+/' | base64 -d; } | od -An -v -tx1 | tr -d ' \n')
    shred -u "$w/half"
    mkdir -p /run/dd
    for k in disk vault; do
      openssl kdf -keylen 32 -kdfopt digest:SHA256 -kdfopt "hexkey:$ikm" \
        -kdfopt "salt:$BOX" -kdfopt "info:dd-$k-v1" -binary HKDF >"/run/dd/$k.key"
    done
    chmod 0400 /run/dd/*.key
    # and for the owner's paper key: a dead TPM, or a firmware update, is
    # not the end of the data
    mkdir -p /boot/dd
    printf '%s\n' "$RECOVERY" >"$w/recipient"
    for k in disk vault; do age -R "$w/recipient" -o "/boot/dd/$BOX-$k.age" "/run/dd/$k.key"; done
    echo "share kept by the Worker; keys in /run/dd; paper-key copies in /boot/dd"
    ;;
  *)
    echo "usage: dd-unlock-enrol keys|share" >&2
    exit 2
    ;;
esac
