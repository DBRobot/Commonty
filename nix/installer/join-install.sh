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
# git+https://...), DETECT (detect.sh),
# NOT_BEFORE (seconds: the oldest release it takes, when it was made),
# DISK (to name the disk when there is more than one; it is wiped),
# Secure Boot in setup mode (its keys cleared at the firmware's screen),
# WIFI ({ssid, psk}, a file) and TICKET (the member's join ticket, a file),
# both from the drive's commonty/ (docs/dev/joining.md).
set -euo pipefail
say() { echo "== $*"; }
no() { echo "not installing: $*" >&2; exit 1; }
# a step that fails says which, never a silent stop
trap 'echo "failed: line $LINENO: $BASH_COMMAND" >&2' ERR

[ -e /dev/tpmrm0 ] || no "this machine has no TPM 2.0: a box's keys live in its TPM"
[ -d /sys/firmware/efi ] || no "this machine did not boot by UEFI"
# Secure Boot in setup mode: the box enrolls keys of its own, and its disk's
# key is sealed under the boot they sign (docs/dev/joining.md)
setupmode=$(od -An -tu1 -j4 -N1 /sys/firmware/efi/efivars/SetupMode-8be4df61-93ca-11d2-aa0d-00e098032b8c 2>/dev/null | tr -d ' ' || true)
[ "$setupmode" = 1 ] || no "Secure Boot is not in setup mode: clear its keys (reset to setup mode) in the firmware settings"

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

# the half its disk key comes from at its first boot, in its TPM, made
# before anything is wiped. It is held under no PCR: this boot's PCR 7 is
# setup mode's, not the signed boot the box will have. Its first boot with
# Secure Boot on seals a new half under that, re-keys the pool, removes
# this one, and makes the box's own key to join with under the same
# (dd-unlock-seal): nothing that names or signs for the box exists before
# its signed boot does
export TPM2TOOLS_TCTI=device:/dev/tpmrm0
mkdir -p "$w/unlock"
tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
( umask 077; head -c 32 /dev/urandom > "$w/half" )
tpm2_create -Q -C "$w/srk.ctx" -a 'fixedtpm|fixedparent|noda|userwithauth' \
  -i "$w/half" -u "$w/unlock/bootstrap.pub" -r "$w/unlock/bootstrap.priv"
tpm2_flushcontext -t >/dev/null 2>&1 || true
# its name comes from its key (state.JoinName), made at its first start;
# until then it is built under this one, and builds itself again named
name=joining

# the disk's key, as its initrd makes it from that half at the first boot
# (disk-unlock-boot.sh, TPM_ONLY): a pulled disk opens nothing
mkdir -p /run/dd
( umask 077
  openssl kdf -keylen 32 -kdfopt digest:SHA256 -kdfopt "hexkey:$(od -An -v -tx1 < "$w/half" | tr -d ' \n')" \
    -kdfopt salt:dd-tpm-only -kdfopt info:dd-disk-v1 -binary HKDF > /run/dd/disk.key )
shred -u "$w/half"

# the layout's script is made, with nothing to evaluate here, for the one
# name dd-target: that name is the disk chosen. Its partitions are found by
# label, so another disk carrying the same labels (an old box's) is refused
for d in "${disks[@]}"; do
  [ "$d" = "$disk" ] && continue
  lsblk -nro PARTLABEL "$d" | grep -q '^disk-boot-' \
    && no "$d holds an old box's disk: take it out, or wipe it first"
done
ln -sfn "$disk" /dev/dd-target
/etc/dd/wipe --yes-wipe-all-disks
shred -u /run/dd/disk.key

mkdir -p /mnt/boot/dd/unlock /mnt/var/lib/dd
cp "$w/unlock/"* /mnt/boot/dd/unlock/
( umask 077; age-plugin-tpm --generate -o /mnt/var/lib/dd/secrets-key.txt )

# its report, for the disk it is on
mkdir -p /mnt/var/lib/dd/box
nixos-generate-config --root /mnt --no-filesystems --show-hardware-config > /mnt/var/lib/dd/box/hardware-configuration.nix
BOOT=/mnt/boot SECRETS_KEY=/mnt/var/lib/dd/secrets-key.txt bash -x "$DETECT" > "$w/hw.json" 2> "$w/detect.log" \
  || { tail -20 "$w/detect.log" >&2; no "the hardware report failed"; }
jq --arg d "$byid" '.bootDisk = $d' "$w/hw.json" > /mnt/var/lib/dd/box/hardware.json
jq -er .secretsKey /mnt/var/lib/dd/box/hardware.json > /mnt/var/lib/dd/secrets-recipient
# the Wi-Fi it was set up on, if any: its password sealed to this box's
# secrets key, which only its TPM opens (host.nix, dd-wifi-unseal)
ssid=""
if [ -n "${WIFI:-}" ] && [ -s "$WIFI" ]; then
  ssid=$(jq -r '.ssid // ""' "$WIFI")
  jq -r '"psk=" + .psk' "$WIFI" \
    | age -r "$(cat /mnt/var/lib/dd/secrets-recipient)" -o /mnt/var/lib/dd/wifi.age
fi
# the member it is set up for: its join ticket, used once on first boot
if [ -n "${TICKET:-}" ] && [ -s "$TICKET" ]; then
  install -m 0600 "$TICKET" /mnt/var/lib/dd/join-ticket
fi
jq -n --arg ssid "$ssid" '{owner: null, house: {ssid: $ssid, address: "", sealedWifi: ($ssid != "")},
  encrypted: true, unlock: "tpm", extraPools: [], datasets: {}}' > /mnt/var/lib/dd/box/box.json

# its system, built here from that commit and its report, onto the disk:
# the installer's own store is memory. What it already has is taken from it
say "building its system from $rev"
sys=$(nix --extra-experimental-features "nix-command flakes" build --impure --no-link --print-out-paths \
  --store /mnt --extra-substituters 'daemon?trusted=true' --expr "let f = builtins.getFlake \"$SOURCE?rev=$rev\"; in (f.lib.joinedBox { name = \"$name\"; dir = builtins.path { path = /mnt/var/lib/dd/box; name = \"box\"; }; }).config.system.build.toplevel")
# its Secure Boot keys, on its encrypted disk (/var/lib/sbctl, where
# lanzaboote signs every release's boot with them), made before the
# install so its boot is signed from the first
mkdir -p /mnt/var/lib/sbctl "$w/sbctl"
printf 'keydir: /mnt/var/lib/sbctl/keys\nguid: /mnt/var/lib/sbctl/GUID\nfiles_db: %s/files.json\nbundles_db: %s/bundles.json\n' \
  "$w/sbctl" "$w/sbctl" > "$w/sbctl.conf"
sbctl --config "$w/sbctl.conf" --disable-landlock create-keys
nixos-install --root /mnt --system "$sys" --no-root-passwd --no-channel-copy
# and enrolled, with Microsoft's beside them as node1's and node2's are:
# firmware drivers on cards (graphics, network) are signed by Microsoft,
# and without its keys some machines start with no screen. A system
# Microsoft's keys start leaves PCR 7 unlike this box's own boot, so the
# disk's key opens for neither it nor anything else
sbctl --config "$w/sbctl.conf" --disable-landlock enroll-keys --microsoft
say "Secure Boot keys of its own enrolled: it starts nothing else from now"
# the release it runs is where its agent starts: one signed before it
# never takes the box back
mkdir -p /mnt/var/lib/dd-agent
echo "$counter $sys" > /mnt/var/lib/dd-agent/applied
echo "$counter" > /mnt/var/lib/dd-agent/counter
say "installed. It joins the fleet on its first start, on its own."
