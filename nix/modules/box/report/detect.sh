# What this box finds on itself, as fleet/boxes/<box>/hardware.json reads it
# (modules/box/host.nix): its boot disk by id, its links, its GPU, its host id
set -eu
disk=$(lsblk -no PKNAME "$(findmnt -no SOURCE /boot)")
boot=""
for l in /dev/disk/by-id/*; do
  [ "$(readlink -f "$l")" = "/dev/$disk" ] || continue
  n=$(basename "$l")
  case "$n" in nvme-eui.*|wwn-*|*_1) continue ;; esac
  [ -z "$boot" ] || [ ${#n} -lt ${#boot} ] && boot=$n
done
wired=null; wifi=null; usb=false
for d in /sys/class/net/*; do
  [ -e "$d/device" ] || continue
  i=$(basename "$d")
  if [ -e "$d/wireless" ] || [ -e "$d/phy80211" ]; then wifi="\"$i\""; continue; fi
  [ "$(cat $d/type)" = 1 ] || continue
  wired="\"$(tr a-f A-F < $d/address)\""
  case "$(readlink -f $d/device)" in */usb*) usb=true ;; esac
done
gpu=null
for r in /sys/class/drm/renderD*; do
  [ -e "$r/device/driver" ] || continue
  drv=$(basename "$(readlink -f $r/device/driver)")
  case "$drv" in i915|xe) drv=intel ;; esac
  gpu="{ \"driver\": \"$drv\", \"render\": \"/dev/dri/$(basename $r)\" }"; break
done
# what its secrets are encrypted to, sops's form of it (the older recipient
# kind: sops cannot encrypt to the newer one yet)
secrets=null
if [ -n "${SECRETS_KEY:-}" ] && [ -e "$SECRETS_KEY" ]; then
  secrets="\"$(age-plugin-tpm -y --tpm-recipient "$SECRETS_KEY")\""
fi
printf '{\n  "hostId": "%s",\n  "bootDisk": "/dev/disk/by-id/%s",\n  "wired": %s,\n  "wifi": %s,\n  "gpu": %s,\n  "usbNic": %s,\n  "secretsKey": %s\n}\n' "$(hostid)" "$boot" "$wired" "$wifi" "$gpu" "$usb" "$secrets"
