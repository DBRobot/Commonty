{ config, ... }:
{
  # What is true of this machine and no other. Everything it runs is a role
  # in fleet/boxes.json. hardware-configuration.nix is the output of
  # `nixos-generate-config --no-filesystems --show-hardware-config` on the
  # box, unedited: disko owns the mounts, the generator owns the rest.
  imports = [
    ./hardware-configuration.nix
    ./disko.nix
    ../../modules/box/home-network.nix
  ];

  # the house network: this box's links and its reserved address
  # the disks' key made in the initrd at every boot, from this box's TPM
  # and the unlock Worker (modules/box/disk-unlock.nix); enrolled 2026-10-03
  dd.unlock.enable = true;

  dd.home = {
    ssid = "Zyxel05804";
    pskFile = config.sops.templates."wifi.env".path;
    address = "192.168.1.21/24";
    wifi = "wlp2s0";
    # the ethernet adapter is on the switch, and the switch is on the router
    wired = "00:E0:4C:3A:46:58";
  };
  sops.secrets.wifi-psk = { };
  sops.templates."wifi.env".content = "psk=${config.sops.placeholder.wifi-psk}\n";

  # root is on tank, so the pool imports at boot on its own

  networking.hostId = "083f7ed3";

  # garage's blocks on the one pool there is, encrypted with the rest.
  # Content-addressed ciphertext: no snapshots, nothing to gain from them
  dd.zfs.datasets."tank/enc/garage" = {
    mountpoint = "/srv/garage";
    recordsize = "1M";
    "com.sun:auto-snapshot" = "false";
  };

  # no suspend on close, on battery or not, no idle action, and the USB
  # ethernet adapter must never autosuspend - it is the only way in
  services.logind.settings.Login = {
    HandleLidSwitchDocked = "ignore";
    HandlePowerKey = "ignore";
    IdleAction = "ignore";
  };
  systemd.sleep.settings.Sleep = {
    AllowSuspend = "no";
    AllowHibernation = "no";
    AllowHybridSleep = "no";
    AllowSuspendThenHibernate = "no";
  };
  boot.kernelParams = [ "usbcore.autosuspend=-1" ];
}
