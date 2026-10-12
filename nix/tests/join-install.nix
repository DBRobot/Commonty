# The installer for a box joining the fleet (nix/installer): on a machine
# with a TPM and two blank disks it refuses to choose between them, then,
# told which, wipes that one alone; makes the box's key in its TPM and its
# name from it, its secrets key and report; installs the system the
# release's commit builds for it. The commit is a stand-in flake here,
# whose system is one already in the store, so nothing is fetched.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  # what the stand-in commit builds for the box: a small system, already in
  # the machine's store, so nothing is fetched
  installed =
    (pkgs.nixos {
      boot.loader.grub.enable = false;
      fileSystems."/" = {
        device = "tank/root";
        fsType = "zfs";
      };
      networking.hostId = "dd000001";
      system.stateVersion = "26.05";
    }).config.system.build.toplevel;
in
{
  name = "join-install";
  node.specialArgs = { inherit self; };
  nodes = {
    forge = {
      services.nginx = {
        enable = true;
        virtualHosts.forge.root = "/srv";
      };
      networking.firewall.allowedTCPPorts = [ 80 ];
      environment.systemPackages = [ pkgs.git ];
    };
    blank =
      { config, ... }:
      {
        imports = [ ../installer/iso.nix ];
        virtualisation.tpm.enable = true;
        virtualisation.useEFIBoot = true;
        virtualisation.emptyDiskImages = [
          8192
          8192
        ];
        virtualisation.memorySize = 2048;
        system.extraDependencies = [ installed ];
        environment.etc."installed".text = "${installed}";
        environment.variables = lib.mkForce {
          RELEASE_URL = "http://forge/release/current.json";
          RELEASE_KEY = "/root/release.pub";
          SOURCE = "git+http://forge/flake.git";
          LAYOUT = "${../../fleet/layout.nix}";
          DETECT = "${../modules/box/report/detect.sh}";
        };
      };
  };
  scriptEnv = { inherit dd; };
}
