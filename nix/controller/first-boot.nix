# The seed's one job: lock the rest of the card and install the controller
# into it. It finds the house box's Tang on the LAN among the fleet's
# (fleet/tang.json), and takes one only if its key is the thumbprint listed
# there: a Tang anyone else stands up on the LAN is never used. The LUKS key
# is random, lives only in memory, and is dropped once Clevis holds it, so
# the card opens through that Tang and nothing else.
{
  config,
  lib,
  pkgs,
  tang,
  ...
}:
let
  installs = config.dd.controller.installs;
  servers = lib.mapAttrsToList (_: t: t) (lib.filterAttrs (_: t: (t.thp or "") != "") tang);
  pins = pkgs.writeText "tang-pins.json" (builtins.toJSON servers);
in
{
  system.extraDependencies = [ installs ];

  systemd.services.dd-controller-lock = {
    description = "Lock the card and install the home controller into it";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    unitConfig.ConditionPathExists = "!/dev/disk/by-label/controller-root";
    path = [
      pkgs.clevis
      pkgs.jose
      pkgs.curl
      pkgs.cryptsetup
      pkgs.util-linux
      pkgs.e2fsprogs
      pkgs.jq
      pkgs.coreutils
      pkgs.gnugrep
      pkgs.bash
      config.nix.package
      config.system.build.nixos-install
    ];
    serviceConfig = {
      Type = "oneshot";
      Restart = "on-failure";
      RestartSec = 60;
    };
    script = ''
      set -euo pipefail
      seed=$(findmnt -no SOURCE /)
      disk=/dev/$(lsblk -no PKNAME "$seed")
      case "$disk" in *[0-9]) part="''${disk}p3" ;; *) part="''${disk}3" ;; esac
      if [ ! -e "$part" ]; then
        echo ",,L" | sfdisk --append --no-reread "$disk"
        partx -u "$disk" || true
        udevadm settle
      fi
      bash ${./lock.sh} "$part" ${pins} ${installs}
      echo "locked and installed; starting the controller"
      systemctl reboot
    '';
  };
}
