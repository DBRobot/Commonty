# shellcheck shell=bash
# A box that joined, at its first boot with Secure Boot on and its own keys
# enrolled (docs/dev/joining.md): a new half of the disk key, sealed in the
# TPM under this boot's PCR 7, the pool re-keyed to what it makes, and the
# installer's half, which no PCR held, gone. From now the TPM opens the
# disk only under this box's signed boot: not a USB drive's system, not its
# own with a changed command line, not with Secure Boot off or other keys.
# From the unit: POOL (the pool whose encryption root is POOL/enc).
set -euo pipefail
umask 077
dir=/boot/dd/unlock
[ -e "$dir/bootstrap.priv" ] || exit 0
# stopped after the re-key last time: only the files are left to move
if [ -e "$dir/rekeyed" ]; then
  mv "$dir/half.pub.new" "$dir/half.pub" && mv "$dir/half.priv.new" "$dir/half.priv"
  shred -u "$dir/bootstrap.pub" "$dir/bootstrap.priv"
  rm -f "$dir/rekeyed"
  sync
  exit 0
fi

efivar() {
  local f=/sys/firmware/efi/efivars/$1-8be4df61-93ca-11d2-aa0d-00e098032b8c
  if [ -r "$f" ]; then od -An -tu1 -j4 -N1 "$f" | tr -d ' '; fi
}
# sealed under anything less, the half would open for any system started
# here: wait, keep the installer's half, and say why
if [ "$(efivar SecureBoot)" != 1 ] || [ "$(efivar SetupMode)" != 0 ]; then
  echo "dd-unlock-seal: Secure Boot is not on with this box's keys; the disk's key stays the installer's until it is" >&2
  exit 0
fi

w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
export TPM2TOOLS_TCTI=device:/dev/tpmrm0
head -c 32 /dev/urandom >"$w/half"
tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
tpm2_startauthsession -Q -S "$w/s.ctx"
tpm2_policypcr -Q -S "$w/s.ctx" -l sha256:7 -L "$w/pcr.pol"
tpm2_flushcontext "$w/s.ctx"
tpm2_create -Q -C "$w/srk.ctx" -L "$w/pcr.pol" -a 'fixedtpm|fixedparent|noda' \
  -i "$w/half" -u "$w/half.pub" -r "$w/half.priv"
tpm2_flushcontext -t >/dev/null 2>&1 || true

# the key the initrd will make from it (disk-unlock-boot.sh, TPM_ONLY)
mkdir -p /run/dd-seal
openssl kdf -keylen 32 -kdfopt digest:SHA256 \
  -kdfopt "hexkey:$(od -An -v -tx1 <"$w/half" | tr -d ' \n')" \
  -kdfopt salt:dd-tpm-only -kdfopt info:dd-disk-v1 -binary HKDF >/run/dd-seal/disk.key
shred -u "$w/half"

# the new half on the boot partition, the pool re-keyed to it, then a
# mark that it was: a stop anywhere leaves the initrd knowing which half
# opens the pool (disk-unlock-boot.sh)
rm -f "$dir/rekeyed"
cp "$w/half.pub" "$dir/half.pub.new" && cp "$w/half.priv" "$dir/half.priv.new"
sync
zfs change-key -o keyformat=raw -o keylocation=file:///run/dd-seal/disk.key "$POOL/enc"
zfs set keylocation=file:///run/dd/disk.key "$POOL/enc"
touch "$dir/rekeyed"
sync
mv "$dir/half.pub.new" "$dir/half.pub" && mv "$dir/half.priv.new" "$dir/half.priv"
shred -u "$dir/bootstrap.pub" "$dir/bootstrap.priv"
rm -f "$dir/rekeyed"
sync
shred -u /run/dd-seal/disk.key
echo "dd-unlock-seal: the disk's key is sealed under this box's signed boot"

# the box's own key to join with, under the same boot (PCR 7), as node1's
# and node2's: made now, never before its signed boot existed. Its name
# comes from it (state.JoinName)
if [ ! -s "$dir/sig.pub" ]; then
  tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
  tpm2_startauthsession -Q -S "$w/s.ctx"
  tpm2_policypcr -Q -S "$w/s.ctx" -l sha256:7 -L "$w/pcr.pol"
  tpm2_flushcontext "$w/s.ctx"
  tpm2_create -Q -C "$w/srk.ctx" -G ecc256:ecdsa-sha256 -L "$w/pcr.pol" \
    -a 'fixedtpm|fixedparent|sensitivedataorigin|sign|noda' \
    -u "$dir/sig.pub" -r "$dir/sig.priv"
  tpm2_load -Q -C "$w/srk.ctx" -u "$dir/sig.pub" -r "$dir/sig.priv" -c "$w/sig.ctx"
  tpm2_readpublic -Q -c "$w/sig.ctx" -f der -o "$w/spki.der"
  tpm2_flushcontext -t >/dev/null 2>&1 || true
  mkdir -p /var/lib/dd
  (umask 022 && base64 -w0 <"$w/spki.der" >/var/lib/dd/box-key.spki)
  sync
  echo "dd-unlock-seal: its key to join with, made under the same boot: j$(sha256sum <"$w/spki.der" | cut -c1-10)"
fi
