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
  # the member's join ticket, as the app writes it to the drive: a biscuit
  # naming its ticket id (client/auth mint_join_ticket). Made on forge when
  # the test runs, which also makes the fake gate take it
  ticketId = "00112233445566778899aabbccddeeff";

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
        # for the test to change PCR 7 under it
        environment.systemPackages = [ pkgs.tpm2-tools ];
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
        virtualisation.useSecureBoot = true;
        virtualisation.useDefaultFilesystems = false;
        # blank's, already there: no image of its own to take them from
        virtualisation.efi.keepVariables = false;
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
        # the ticket it takes is made when the test runs (/run/gate)
        validateConfigFile = false;
        virtualHosts.forge = {
          root = "/srv";
          # the gate's pending backups, as the gate takes them: the
          # ticket's own folder, on the ticket alone, and no deleting
          locations."/files/images/pending/${ticketId}/".extraConfig = ''
            include /run/gate/want.conf;
            if ($http_authorization != $want) { return 403; }
            limit_except GET HEAD PROPFIND PUT MKCOL { deny all; }
            client_max_body_size 0;
            proxy_pass http://127.0.0.1:8081;
          '';
        };
      };
      systemd.tmpfiles.rules = [
        "d /run/gate 0755 root root -"
        "f /run/gate/want.conf 0644 root root - set $want \"none\";"
      ];
      systemd.services.fake-gate = {
        wantedBy = [ "multi-user.target" ];
        preStart = "mkdir -p /srv/gate";
        script = "exec ${pkgs.rclone}/bin/rclone serve webdav --addr 127.0.0.1:8081 --baseurl /files/images/pending /srv/gate";
      };
      networking.firewall.allowedTCPPorts = [ 80 ];
      environment.systemPackages = [
        pkgs.git
        pkgs.restic
        pkgs.rclone
        pkgs.biscuit-cli
      ];
    };
    blank =
      { config, ... }:
      {
        imports = [
          ../installer/iso.nix
          ../installer/setup.nix
        ];
        # the drive's commonty/, as the app wrote it
        systemd.services.dd-box-setup.environment.SETUP_DRIVE = "/root/drive";
        virtualisation.tpm.enable = true;
        virtualisation.useEFIBoot = true;
        # its firmware's Secure Boot in setup mode, no keys yet: the box
        # enrolls its own
        virtualisation.useSecureBoot = true;
        virtualisation.emptyDiskImages = [
          8192
          8192
        ];
        virtualisation.memorySize = 2048;
        system.extraDependencies = [ installed ];
        environment.systemPackages = [
          pkgs.jq
          pkgs.curl
          pkgs.parted
          pkgs.e2fsprogs
        ];
        environment.etc."installed".text = "${installed}";
        environment.variables = lib.mkForce {
          RELEASE_URL = "http://forge/release/current.json";
          RELEASE_KEY = "/root/release.pub";
          SOURCE = "git+http://forge/flake.git";
          DETECT = "${../modules/box/report}/detect.sh";
          NOT_BEFORE = "100";
          FILES = "http://forge/files";
        };
      };
  };
  scriptEnv = { inherit dd ticketId; };
}
