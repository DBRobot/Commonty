# Unlocking the disks at boot: a box with a TPM (swtpm) enrols, encrypts a
# pool to the key, and from then on reboots unlocked by itself at home,
# waits for a member somewhere new, stays locked while marked stolen, and
# opens with the owner's paper key when nothing else will. The unlock
# service is a stand-in speaking the Worker's protocol (unlock/fake-worker.py).
{ pkgs, self, ... }:
let
  python = pkgs.python3.withPackages (ps: [ ps.cryptography ]);
in
{
  name = "unlock";
  nodes = {
    box =
      { lib, ... }:
      {
        imports = [ ../modules/box/disk-unlock.nix ];
        # what home-network.nix gives a real box: the cable, the address, the router
        options.dd.home = lib.mkOption { type = lib.types.attrs; };
        options.dd.domain = lib.mkOption { type = lib.types.str; };
        config = {
          dd.domain = "test.invalid";
          dd.home = {
            wired = "52:54:00:12:01:01";
            address = "192.168.1.1/24";
            gateway = "192.168.1.2";
          };
          dd.unlock = {
            enable = true;
            url = "http://192.168.1.2:8000";
            pools = [ "data" ];
            recovery = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOpKbGPinFIKvvVQexMuxfmVR3auvr57kkIe6mkURtIs test-paper-key";
          };
          virtualisation = {
            tpm.enable = true;
            useBootLoader = true;
            useEFIBoot = true;
            emptyDiskImages = [ 1024 ];
            memorySize = 1536;
          };
          boot.loader.systemd-boot.enable = true;
          boot.loader.efi.canTouchEfiVariables = true;
          boot.supportedFilesystems = [ "zfs" ];
          boot.zfs.extraPools = [ "data" ];
          # a vm disk has no /dev/disk/by-id name to import by
          boot.zfs.devNodes = "/dev";
          networking.hostId = "8425e349";
          networking.firewall.enable = false;
          environment.systemPackages = [
            self.packages.${pkgs.stdenv.hostPlatform.system}.dd
            pkgs.tpm2-tools
          ];
        };
      };
    worker = {
      networking.firewall.enable = false;
      environment.systemPackages = [ pkgs.jq ];
      systemd.services.fake-worker = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${python}/bin/python3 ${./unlock/fake-worker.py}";
      };
    };
  };
  scriptEnv = {
    paper = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
  };
}
