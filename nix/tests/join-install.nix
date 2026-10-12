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
  # the machine's store, so nothing is fetched. It boots as a joined box
  # does: its pool encrypted, the key from its TPM alone, lanzaboote
  installed =
    (pkgs.nixos (
      { modulesPath, ... }:
      {
        imports = [
          "${modulesPath}/testing/test-instrumentation.nix"
          "${modulesPath}/profiles/qemu-guest.nix"
          self.inputs.disko.nixosModules.disko
          self.inputs.lanzaboote.nixosModules.lanzaboote
          (import ../../fleet/layout.nix {
            device = "/dev/vdb";
            encrypted = true;
          })
          ../modules/box/secure-boot.nix
          ../modules/box/disk-unlock.nix
        ];
        _module.args.ddScript = import ../lib/script.nix { inherit lib; };
        networking.hostName = "joined";
        networking.hostId = "dd000001";
        boot.initrd.systemd.enable = true;
        boot.initrd.availableKernelModules = [ "tpm_tis" ];
        dd.unlock = {
          enable = true;
          tpmOnly = true;
          url = "http://unused";
          recovery = "";
          waits.enrolment = 5;
        };
        system.stateVersion = "26.05";
      }
    )).config.system.build.toplevel;
in
{
  name = "join-install";
  node.specialArgs = { inherit self; };
  nodes = {
    # the same machine after the install, booting its disk: blank's state
    # (its TPM, its EFI variables and the disk it installed to)
    target =
      { lib, ... }:
      {
        system.name = "blank";
        virtualisation.useBootLoader = true;
        virtualisation.useEFIBoot = true;
        virtualisation.useDefaultFilesystems = false;
        virtualisation.diskImage = "./empty0.qcow2";
        virtualisation.tpm.enable = true;
        virtualisation.memorySize = 2048;
        # never mounted: it boots what is on the disk; the VM module asks
        virtualisation.fileSystems."/" = {
          device = "tank/enc/system/root";
          fsType = "zfs";
        };
        networking.hostId = "dd000001";
        boot.supportedFilesystems.zfs = true;
      };
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
        environment.systemPackages = [ pkgs.jq ];
        environment.etc."installed".text = "${installed}";
        environment.variables = lib.mkForce {
          RELEASE_URL = "http://forge/release/current.json";
          RELEASE_KEY = "/root/release.pub";
          SOURCE = "git+http://forge/flake.git";
          DETECT = "${../modules/box/report/detect.sh}";
          NOT_BEFORE = "100";
        };
      };
  };
  scriptEnv = { inherit dd; };
}
