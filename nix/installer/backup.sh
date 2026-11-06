# shellcheck shell=bash
# What is on a disk, before the box installer wipes it: every filesystem it
# can read, mounted read-only and backed up file by file with restic into
# the member's pending backup (docs/dev/joining.md), encrypted here with the
# drive's backup.key before anything leaves. File by file, not block by
# block: Windows' NTFS and Linux's filesystems both read that way, a
# backup holds what was used and no free space, and what comes back is
# files to open, not a disk image to restore whole.
# Environment: DISK, TICKET (a file), BACKUP_KEY (a file), FILES (the
# fleet's files address, https://files.<domain>), and NET_DOOR and the rest
# of network.sh's when Files is on the fleet's network.
# Prints restic's --json lines (the screen reads its progress from them).
set -euo pipefail
trap 'echo "failed: line $LINENO: $BASH_COMMAND" >&2' ERR
[ -s "$TICKET" ] || { echo "no join ticket on the drive" >&2; exit 1; }
[ -s "$BACKUP_KEY" ] || { echo "no backup key on the drive" >&2; exit 1; }
id=$(dd-box-setup --ticket-id "$TICKET")

w=$(mktemp -d)
# shellcheck disable=SC2329 # run by the trap
cleanup() {
  for m in "$w"/m/*; do umount "$m" 2>/dev/null || true; done
  zpool export -a 2>/dev/null || true
  [ -z "${NET_DOOR:-}" ] || dd-installer-net down
  rm -rf "$w"
}
trap cleanup EXIT

# every filesystem on the disk, read-only; one it can't read is said and left
mkdir -p "$w/m"
mounts=()
while read -r part fs; do
  [ -n "$fs" ] || continue
  m="$w/m/$(basename "$part")"
  mkdir -p "$m"
  case "$fs" in
    ntfs) ntfs-3g -o ro "$part" "$m" ;;
    ext2 | ext3 | ext4 | xfs | btrfs | vfat | exfat) mount -o ro "$part" "$m" ;;
    zfs_member)
      pool=$(zpool import -d "$part" 2>/dev/null | awk '/pool:/ {print $2; exit}')
      if [ -n "$pool" ] && zpool import -N -o readonly=on -R "$m" -d "$part" "$pool"; then
        zfs mount -a 2>/dev/null || true
      fi
      ;;
    BitLocker) echo "$part is locked with BitLocker: it can't be read from here, and is not backed up" >&2; continue ;;
    *) echo "$part ($fs) can't be read from here, and is not backed up" >&2; continue ;;
  esac
  mountpoint -q "$m" && mounts+=("$m")
done < <(lsblk -nrpo NAME,FSTYPE "$DISK" | tail -n +2)
[ "${#mounts[@]}" -gt 0 ] || { echo "nothing on $DISK to back up" >&2; exit 0; }

# Files is on the fleet's network: the installer joins it for this alone
# (network.sh), and the whole ticket goes only over it
[ -z "${NET_DOOR:-}" ] || dd-installer-net up

# the member's pending backup, at the gate, on the ticket alone: it opens
# that folder and nothing else of theirs
export RCLONE_CONFIG_PENDING_TYPE=webdav
export RCLONE_CONFIG_PENDING_URL="$FILES/_dd/images/pending/$id"
export RCLONE_CONFIG_PENDING_VENDOR=other
RCLONE_CONFIG_PENDING_BEARER_TOKEN=$(tr -d '\n' < "$TICKET")
export RCLONE_CONFIG_PENDING_BEARER_TOKEN
export RESTIC_REPOSITORY=rclone:pending:
export RESTIC_PASSWORD_FILE=$BACKUP_KEY
restic cat config >/dev/null 2>&1 || restic init >&2

# a dropped connection is tried again: what was sent stays sent
for try in 1 2 3 4 5; do
  if restic backup --json --host "$(basename "$DISK")" --tag "from-installer" "${mounts[@]}"; then
    exit 0
  fi
  echo "the connection dropped; trying again ($try)" >&2
  sleep $((try * 10))
done
exit 1
