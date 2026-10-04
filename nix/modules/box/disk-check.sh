# Is this the box's own pool, before anything on it runs? The key the
# initrd made opens a pool whoever's it is: swap in a disk whose pool has
# the same name and an unencrypted root, and the box would boot it and hand
# it the key and the TPM (oddlama.org, "bypassing disk encryption with tpm2
# unlock"). Only a pool our key actually opened passes: every dataset in it
# but the pool itself under <pool>/enc, with the key loaded. If not, the
# keys go, PCR 7 is spoilt until the next boot so the TPM opens nothing
# more, and this fails, which stops what waits on it.
set -u
refuse() {
  echo "dd-disk-check: $1: not this box's pool; the keys are gone until the next boot" >&2
  shred -u /run/dd/*.key 2>/dev/null || true
  tpm2_pcrextend 7:sha256=0000000000000000000000000000000000000000000000000000000000000000 2>/dev/null || true
  exit 1
}
for pool in "$@"; do
  # not imported (not made yet, or its disk is out): nothing of it mounts
  if ! zpool list -H -o name "$pool" >/dev/null 2>&1; then
    echo "dd-disk-check: $pool is not here"
    continue
  fi
  zfs list -H -o name "$pool/enc" >/dev/null 2>&1 || refuse "$pool has no $pool/enc"
  list=$(zfs list -H -r -o name,mountpoint,encryptionroot,keystatus -t filesystem,volume "$pool") || refuse "$pool cannot be listed"
  while IFS="$(printf '\t')" read -r name mp root status; do
    if [ "$name" = "$pool" ]; then
      case "$mp" in none | "/$pool") continue ;; esac
      refuse "$pool is mounted at $mp"
    fi
    [ "$root" = "$pool/enc" ] || refuse "$name is not under $pool/enc"
    [ "$status" = available ] || refuse "$name: its key did not open it"
  done <<<"$list"
  echo "dd-disk-check: $pool is this box's own"
done
