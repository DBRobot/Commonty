# shellcheck shell=bash
# Lock a partition to the house's Tang and, given a system, install it there
# (first-boot.nix; the home-controller vm test runs it on a spare disk).
#   lock.sh <partition> <tang-pins.json> [<system to install>]
# The pins are the fleet's Tangs, each {url, thp}: the first that answers
# here is bound, and only with its own key's thumbprint, so a Tang anyone
# else stands up on the network is never used. The LUKS key is random, lives
# only in memory, and is dropped once Clevis holds it.
set -euo pipefail
part=$1
pins=$2
system=${3:-}

# a Tang counts only if what it advertises is signed by its listed key:
# clevis refuses to encrypt to it otherwise
tang=""
for i in $(seq 0 $(($(jq length "$pins") - 1))); do
  try=$(jq -c ".[$i] | {url, thp}" "$pins")
  if echo probe | timeout 15 clevis encrypt tang "$try" >/dev/null 2>&1; then
    tang=$try
    break
  fi
done
if [ -z "$tang" ]; then
  echo "no Tang of the fleet's on this network yet"
  exit 1
fi

key=$(mktemp -p /run)
trap 'shred -u "$key" 2>/dev/null || rm -f "$key"' EXIT
head -c 64 /dev/urandom >"$key"
cryptsetup luksFormat --type luks2 --label controller-root --batch-mode --key-file "$key" "$part"
clevis luks bind -y -k "$key" -d "$part" tang "$tang"
cryptsetup luksRemoveKey "$part" "$key"
# it opens the way every boot will, or nothing more is done
clevis luks unlock -d "$part" -n root
mkfs.ext4 -q -L controller /dev/mapper/root

if [ -n "$system" ]; then
  mkdir -p /mnt
  mount /dev/mapper/root /mnt
  mkdir -p /mnt/boot /mnt/seed
  mount --bind /boot /mnt/boot
  nixos-install --system "$system" --root /mnt --no-root-passwd --no-channel-copy
  umount /mnt/boot /mnt
fi
cryptsetup close root
echo "locked to $(jq -r .url <<<"$tang")"
