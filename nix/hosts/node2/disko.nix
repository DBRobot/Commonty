# node2's disks, through the fleet template: one NVMe, ZFS root.
import ../../../fleet/layout.nix {
  device = "/dev/disk/by-id/nvme-Micron_2450_MTFDKBA512TFK_2311403FB25B";
  # everything under tank/enc, keyed at boot by the TPM and the unlock
  # Worker (2026-10-03)
  encrypted = true;
}
