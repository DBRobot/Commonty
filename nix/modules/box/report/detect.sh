# What this box finds on itself, as fleet/boxes/<box>/hardware.json reads it
# (modules/box/host.nix) and placement will (docs/hardware.md, section 1):
# what never changes while it runs. What does (free space, heat, the
# clock's offset) goes in its heartbeats instead.
# Env: SECRETS_KEY (the TPM key's file), INITRD_MODULES (space separated).
set -eu
cls=/sys/class

# --- what host.nix reads -----------------------------------------------
disk=$(lsblk -no PKNAME "$(findmnt -no SOURCE /boot)")
# a disk's stable name: its shortest by-id link that is not an alias
byid() {
  local best="" n l
  for l in /dev/disk/by-id/*; do
    [ "$(readlink -f "$l")" = "/dev/$1" ] || continue
    n=$(basename "$l")
    case "$n" in nvme-eui.* | wwn-* | *_1) continue ;; esac
    if [ -z "$best" ] || [ ${#n} -lt ${#best} ]; then best=$n; fi
  done
  echo "$best"
}
boot=$(byid "$disk")
wired=null; wifi=null; usb=false
for d in $cls/net/*; do
  [ -e "$d/device" ] || continue
  i=$(basename "$d")
  if [ -e "$d/wireless" ] || [ -e "$d/phy80211" ]; then wifi="\"$i\""; continue; fi
  [ "$(cat "$d/type")" = 1 ] || continue
  wired="\"$(tr a-f A-F < "$d/address")\""
  case "$(readlink -f "$d/device")" in */usb*) usb=true ;; esac
done
gpu=null
for r in $cls/drm/renderD*; do
  [ -e "$r/device/driver" ] || continue
  drv=$(basename "$(readlink -f "$r/device/driver")")
  case "$drv" in i915 | xe) drv=intel ;; esac
  gpu="{ \"driver\": \"$drv\", \"render\": \"/dev/dri/$(basename "$r")\" }"
  break
done
# what its secrets are encrypted to, sops's form of it (the older recipient
# kind: sops cannot encrypt to the newer one yet)
secrets=null
if [ -n "${SECRETS_KEY:-}" ] && [ -e "$SECRETS_KEY" ]; then
  secrets="\"$(age-plugin-tpm -y --tpm-recipient "$SECRETS_KEY")\""
fi

# --- firmware ------------------------------------------------------------
efivar() { # an EFI global variable's one-byte value, or empty
  local f=/sys/firmware/efi/efivars/$1-8be4df61-93ca-11d2-aa0d-00e098032b8c
  [ -r "$f" ] && od -An -tu1 -j4 -N1 "$f" | tr -d ' '
}
uefi=false; [ -d /sys/firmware/efi ] && uefi=true
sb=$(efivar SecureBoot); setup=$(efivar SetupMode)
tpmv=null; ppi=null; ecc=false
if [ -r $cls/tpm/tpm0/tpm_version_major ]; then
  tpmv=$(cat $cls/tpm/tpm0/tpm_version_major)
  [ -r $cls/tpm/tpm0/ppi/version ] && ppi="\"$(cat $cls/tpm/tpm0/ppi/version)\""
  tpm2_getcap ecc-curves 2>/dev/null | grep -q NIST_P256 && ecc=true
fi
# a Windows license in the firmware (ACPI MSDM, OEM activation): whether
# there is one, never the key - this report is public, and the key is a
# license anyone who read it could use. A service that needs it reads it on
# this box when it does.
winkey=false
[ -e /sys/firmware/acpi/tables/MSDM ] && winkey=true
# BIOS settings Linux can change (think-lmi, dell-wmi-sysman, hp-bioscfg),
# and which of them it offers
settings=$(for d in $cls/firmware-attributes/*/attributes; do [ -d "$d" ] && ls "$d"; done 2>/dev/null | grep -v '^pending_reboot$' | jq -R . | jq -s .)
settingsby=$(ls $cls/firmware-attributes 2>/dev/null | jq -R . | jq -s .)

# --- cpu, memory, virtualization ----------------------------------------
model=$(sed -n 's/^model name\s*: //p' /proc/cpuinfo | head -1)
vendor=$(sed -n 's/^vendor_id\s*: //p' /proc/cpuinfo | head -1)
flags=$(sed -n 's/^flags\s*: //p' /proc/cpuinfo | head -1 | tr ' ' '\n' |
  grep -xE 'avx|avx2|avx512f|avx512bw|avx512vl|avx512_vnni|avx_vnni|amx_tile|sha_ni|aes|vmx|svm' | jq -R . | jq -s .)
kvm=false; [ -c /dev/kvm ] && kvm=true
nested=false
for n in /sys/module/kvm_intel/parameters/nested /sys/module/kvm_amd/parameters/nested; do
  [ -r "$n" ] && case "$(cat "$n")" in Y | 1) nested=true ;; esac
done
mem=$(awk '/^MemTotal:/ {print $2 * 1024}' /proc/meminfo)

# every GPU and what video it can encode (transcode wants H.264 encode);
# vainfo finds the driver only where the box has graphics enabled
gpus=$(for r in $cls/drm/renderD*; do
  [ -e "$r/device/driver" ] || continue
  node=/dev/dri/$(basename "$r")
  enc=$(vainfo --display drm --device "$node" 2>/dev/null | sed -n 's/^[[:space:]]*VAProfile\([A-Za-z0-9]*\)[[:space:]]*:[[:space:]]*VAEntrypointEncSlice.*/\1/p' | sort -u | jq -R . | jq -s .)
  jq -nc --arg render "$node" --arg driver "$(basename "$(readlink -f "$r/device/driver")")" \
    --arg vendor "$(cat "$r/device/vendor")" --argjson encode "$enc" \
    '{render: $render, driver: $driver, vendor: $vendor, encode: $encode}'
done | jq -s .)

# --- disks and pools -----------------------------------------------------
disks=$(lsblk -dbnJ -o NAME,TRAN,ROTA,SIZE,TYPE | jq -c '.blockdevices[] | select(.type == "disk" and .size > 0 and (.name | test("^(zd|loop|ram)") | not))' |
  while read -r d; do
    name=$(jq -r .name <<<"$d")
    jq -c --arg id "$(byid "$name")" '{
      id: ("/dev/disk/by-id/" + $id),
      kind: (if .tran == "nvme" then "nvme" elif .rota then "hdd" else "ssd" end),
      usb: (.tran == "usb"),
      bytes: .size }' <<<"$d"
  done | jq -s .)
pools=$(zpool status -jP --json-int 2>/dev/null | jq -c '[.pools[] | {
  name,
  vdevs: [.vdevs[.name].vdevs // {} | to_entries[] | .value | {
    type: .vdev_type,
    disks: (if .vdev_type == "disk" then [.path] else [(.vdevs // {}) | to_entries[] | .value.path] end) }]
  }]' || echo '[]')
pools=$(jq -c '.[]' <<<"$pools" | while read -r p; do
  n=$(jq -r .name <<<"$p")
  jq -c --argjson bytes "$(zpool list -Hp -o size "$n")" \
    --arg enc "$(zfs list -H -o name "$n/enc" 2>/dev/null)" \
    '. + { bytes: $bytes, encrypted: ($enc != ""),
      redundancy: ([.vdevs[].type] | if all(. == "disk") then "none" else (map(select(. != "disk")) | unique | join(",")) end) }' <<<"$p"
done | jq -s .)

# --- network links -------------------------------------------------------
nics=$(for d in $cls/net/*; do
  [ -e "$d/device" ] || continue
  i=$(basename "$d"); drv=$(basename "$(readlink -f "$d/device/driver")")
  wl=false; { [ -e "$d/wireless" ] || [ -e "$d/phy80211" ]; } && wl=true
  u=false; case "$(readlink -f "$d/device")" in */usb*) u=true ;; esac
  ini=false; case " ${INITRD_MODULES:-} " in *" $drv "*) ini=true ;; esac
  jq -nc --arg name "$i" --arg mac "$(tr a-f A-F < "$d/address")" --arg driver "$drv" \
    --argjson wireless $wl --argjson usb $u --argjson initrd $ini \
    '{name: $name, mac: $mac, driver: $driver, wireless: $wireless, usb: $usb, initrd: $initrd}'
done | jq -s .)

# --- power -----------------------------------------------------------------
battery=false; ac=false
for p in $cls/power_supply/*; do
  case "$(cat "$p/type" 2>/dev/null)" in
    Battery) battery=true ;;
    Mains) ac=true ;;
  esac
done

jq -n \
  --arg hostId "$(hostid)" --arg bootDisk "/dev/disk/by-id/$boot" \
  --argjson wired "$wired" --argjson wifi "$wifi" --argjson gpu "$gpu" \
  --argjson usbNic $usb --argjson secretsKey "$secrets" \
  --argjson uefi $uefi --arg sb "$sb" --arg setup "$setup" \
  --argjson tpm "$tpmv" --argjson ecc $ecc --argjson ppi "$ppi" \
  --argjson settingsBy "$settingsby" --argjson settings "$settings" \
  --argjson winkey $winkey \
  --arg arch "$(uname -m)" --arg vendor "$vendor" --arg model "$model" \
  --argjson threads "$(nproc)" --argjson flags "$flags" \
  --argjson kvm $kvm --argjson nested $nested --argjson mem "$mem" \
  --argjson gpus "$gpus" --argjson disks "$disks" --argjson pools "$pools" --argjson nics "$nics" \
  --argjson battery $battery --argjson ac $ac '
{
  hostId: $hostId, bootDisk: $bootDisk, wired: $wired, wifi: $wifi, gpu: $gpu,
  usbNic: $usbNic, secretsKey: $secretsKey,
  firmware: {
    uefi: $uefi,
    secureBoot: ($sb == "1"), setupMode: ($setup == "1"),
    tpm: (if $tpm == null then null else { version: $tpm, p256: $ecc, ppi: $ppi } end),
    settingsFromLinux: (if $settingsBy == [] then null else { by: $settingsBy, offers: $settings } end),
    windowsKey: $winkey
  },
  cpu: { arch: $arch, vendor: $vendor, model: $model, threads: $threads, flags: $flags },
  memoryBytes: $mem,
  virt: { kvm: $kvm, nested: $nested },
  gpus: $gpus,
  disks: $disks, pools: $pools, nics: $nics,
  power: { battery: $battery, mains: $ac }
}'
