# A remote computer's /home disk, nightly: read from a ZFS snapshot into its
# own repository in the box's bucket, the snapshot gone after, and skipped,
# and said, when the store is short of room. The copy's guard is the store
# test's.
{ pkgs, self, ... }:
let
  password = pkgs.writeText "restic-password" "a-password\n";
in
{
  name = "computer-backup";
  helpers = true;
  scriptEnv.password = "${password}";
  node.specialArgs = { inherit self; };
  nodes.a =
    { lib, ... }:
    {
      dd.verify.enable = false;
      imports = [
        ./box.nix
        ../modules/storage/store.nix
        ../modules/storage/backup.nix
        ../modules/computers/backup.nix
      ];
      networking.hostName = lib.mkForce "a";
      networking.hostId = "8425e349";
      boot.supportedFilesystems = [ "zfs" ];
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 4096;
      virtualisation.emptyDiskImages = [ 1024 ];
      dd.store = {
        enable = true;
        capacity = "200M";
        identity = "/etc/age-identity";
        agePlugins = [ ];
        boxes = [ "a" ];
        copyFloorBytes = 1048576;
      };
      # test key only
      environment.etc."age-identity" = {
        text = "AGE-SECRET-KEY-1RYQV6YMT4DPUURXLGDHLEP6P4QLM4NYQ4Z4E594SKSVYTXHZTF6QAJ94TJ";
        mode = "0400";
      };
      dd.backup = {
        paths = [ "/etc/hostname" ];
        passwordFile = password;
      };
      environment.systemPackages = [
        pkgs.restic
        pkgs.zfs
      ];
      systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
    };
}
