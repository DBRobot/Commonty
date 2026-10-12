# shellcheck shell=bash
# A blank machine into a box of the fleet, with nothing set up by hand
# (docs/dev/joining.md). Booted from the installer, it: finds the one disk
# to install to - the internal one, never what it booted from, and refuses
# when that is not plain; makes the box's keys in its TPM (its key to join
# with, its secrets key) and its report; asks the fleet for the release
# and checks the release key signed it; builds the box's own system from
# that commit and its report; installs it. On first boot the box joins the
# fleet on its own key, as a stranger, and takes releases from then on.
# Environment: RELEASE_URL, RELEASE_KEY (a file), SOURCE (the repository,
# git+https://...), LAYOUT (fleet/layout.nix), DETECT (detect.sh),
# NOT_BEFORE (seconds: the oldest release it takes, when it was made),
# DISK (to name the disk when there is more than one; it is wiped).
set -euo pipefail
say() { echo "== $*"; }
no() { echo "not installing: $*" >&2; exit 1; }

[ -e /dev/tpmrm0 ] || no "this machine has no TPM 2.0: a box's keys live in its TPM"
[ -d /sys/firmware/efi ] || no "this machine did not boot by UEFI"

# the disk: internal, not removable, not on usb, and nothing of it mounted
# - never what this installer, or anything else, runs from
mounted() { lsblk -nrpo MOUNTPOINTS "$1" | grep -q .; }
mapfile -t disks < <(lsblk -dnpo NAME,TYPE,RM,TRAN | awk '$2 == "disk" && $3 == 0 && $4 != "usb" { print $1 }' \
  | while read -r d; do mounted "$d" || echo "$d"; done)
if [ -n "${DISK:-}" ]; then
  printf '%s\n' "${disks[@]}" | grep -qx -- "$DISK" || no "$DISK is not an internal disk here: ${disks[*]:-none}"
  disk=$DISK
elif [ "${#disks[@]}" -eq 1 ]; then
  disk=${disks[0]}
else
  no "${#disks[@]} internal disks (${disks[*]:-none}): say which with DISK=, it is wiped"
fi
# a disk with no serial (a VM's) has no by-id name: then its own
byid=$(find /dev/disk/by-id -lname "*/$(basename "$disk")" 2>/dev/null | grep -v -- '-part' | sort | head -1 || true)
[ -n "$byid" ] || byid=$disk
say "installing to $disk ($byid); everything on it goes"

# the release, as the release key signed it: the commit to build
# not one issued before the commit this installer was made from: an old
# release, signed as it was, never installs old code on a new box
read -r rev counter < <(dd-agent --url "$RELEASE_URL" --key-file "$RELEASE_KEY" --state /tmp/dd-agent \
  --print-rev --not-before "$NOT_BEFORE") || true
[ -n "${counter:-}" ] || no "no release the release key signed at $RELEASE_URL"
say "the fleet's release is commit $rev"

w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
# its layout, unencrypted: what a box that joined on its own key keeps is
# ciphertext already
printf 'import %s { device = "%s"; encrypted = false; }\n' "$LAYOUT" "$byid" > "$w/disko.nix"
disko --mode destroy,format,mount --yes-wipe-all-disks "$w/disko.nix"

# its keys, in its TPM: the one it joins with, usable only under its
# signed boot (PCR 7), and its secrets key
mkdir -p /mnt/boot/dd/unlock /mnt/var/lib/dd
export TPM2TOOLS_TCTI=device:/dev/tpmrm0
tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
tpm2_startauthsession -Q -S "$w/s.ctx"
tpm2_policypcr -Q -S "$w/s.ctx" -l sha256:7 -L "$w/pcr.pol"
tpm2_flushcontext "$w/s.ctx"
tpm2_create -Q -C "$w/srk.ctx" -G ecc256:ecdsa-sha256 -L "$w/pcr.pol" \
  -a 'fixedtpm|fixedparent|sensitivedataorigin|sign|noda' \
  -u /mnt/boot/dd/unlock/sig.pub -r /mnt/boot/dd/unlock/sig.priv
tpm2_load -Q -C "$w/srk.ctx" -u /mnt/boot/dd/unlock/sig.pub -r /mnt/boot/dd/unlock/sig.priv -c "$w/sig.ctx"
tpm2_readpublic -Q -c "$w/sig.ctx" -f der -o "$w/spki.der"
tpm2_flushcontext -t >/dev/null 2>&1 || true
( umask 077; age-plugin-tpm --generate -o /mnt/var/lib/dd/secrets-key.txt )
# its name, from its key alone (state.JoinName): j and the first five
# bytes of the key's sha256
name=j$(sha256sum < "$w/spki.der" | cut -c1-10)
base64 -w0 < "$w/spki.der" > /mnt/var/lib/dd/box-key.spki
say "this box joins the fleet as $name"

# its report, for the disk it is on
mkdir -p /mnt/var/lib/dd/box
nixos-generate-config --root /mnt --no-filesystems --show-hardware-config > /mnt/var/lib/dd/box/hardware-configuration.nix
SECRETS_KEY=/mnt/var/lib/dd/secrets-key.txt bash "$DETECT" \
  | jq --arg d "$byid" '.bootDisk = $d' > /mnt/var/lib/dd/box/hardware.json
jq -er .secretsKey /mnt/var/lib/dd/box/hardware.json > /mnt/var/lib/dd/secrets-recipient
jq -n '{owner: null, house: {ssid: "", address: ""}, encrypted: false, extraPools: [], datasets: {}}' \
  > /mnt/var/lib/dd/box/box.json

# its system, built here from that commit and its report
say "building its system from $rev"
sys=$(nix --extra-experimental-features "nix-command flakes" build --impure --no-link --print-out-paths \
  --store /mnt --expr "let f = builtins.getFlake \"$SOURCE?rev=$rev\"; in (f.lib.joinedBox { name = \"$name\"; dir = builtins.path { path = /mnt/var/lib/dd/box; name = \"box\"; }; }).config.system.build.toplevel")
nixos-install --root /mnt --system "$sys" --no-root-passwd --no-channel-copy
# the release it runs is where its agent starts: one signed before it
# never takes the box back
mkdir -p /mnt/var/lib/dd-agent
echo "$counter $sys" > /mnt/var/lib/dd-agent/applied
echo "$counter" > /mnt/var/lib/dd-agent/counter
say "installed. Take the installer out and restart: $name joins the fleet on its own."
