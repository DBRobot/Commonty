# node1's boot disk, through the fleet template: the 1T NVMe, ZFS root.
# vault, the 4T usb disk, is its own pool and not disko's: the template
# never formats a disk it is not told about.
import ../../../fleet/layout.nix {
  device = "/dev/disk/by-id/nvme-INTEL_SSDPEKNW010T8H_BTNH11901JBE1P0B";
  # everything under tank/enc, keyed at boot by the TPM and the unlock
  # Worker (2026-10-04)
  encrypted = true;
}
