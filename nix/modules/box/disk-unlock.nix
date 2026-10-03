# The disks' key at boot, from two halves: one sealed in this box's TPM
# (opened only under its own signed boot, PCR 7), one held by the unlock
# Worker at Cloudflare and handed only to this TPM's signature from home
# (client/mail/worker/unlock.js). No person is needed at a reboot at home; a
# box started anywhere else waits for a member to let it in, and a pulled
# disk or a box carried off opens nothing.
#
# Enrolment, once per box after Secure Boot is on: dd-unlock-enrol keys
# (its files go to the boot partition, /boot/dd/unlock - they open nothing
# outside this TPM), the printed key into fleet/boxes.json (unlockKey), a
# release, then dd-unlock-enrol share. The pools' datasets then live under
# an encryption root whose keylocation is file:///run/dd/disk.key (vault:
# vault.key).
{
  config,
  lib,
  pkgs,
  ddScript,
  ...
}:
let
  cfg = config.dd.unlock;
  box = config.networking.hostName;
  home = config.dd.home;
  tools = [
    pkgs.tpm2-tools
    pkgs.curl
    pkgs.jq
    pkgs.openssl
    pkgs.iproute2
    pkgs.iputils
    pkgs.coreutils
    pkgs.gawk
    pkgs.gnused
    pkgs.util-linux
  ];
  boot = pkgs.writeShellScript "dd-unlock-boot" (
    ddScript ./disk-unlock-boot.sh {
      BOX = box;
      URL = cfg.url;
      OUT = "/run/dd";
    }
  );
  enrol = pkgs.writeShellApplication {
    name = "dd-unlock-enrol";
    runtimeInputs = tools ++ [
      pkgs.age
      pkgs.systemd
    ];
    text = ddScript ./disk-unlock-enrol.sh {
      BOX = box;
      URL = cfg.url;
      DIR = "/boot/dd/unlock";
      RECOVERY = cfg.recovery;
    };
  };
in
{
  options.dd.unlock = {
    url = lib.mkOption {
      type = lib.types.str;
      default = "https://mail.${config.dd.domain}";
      description = "where the unlock Worker answers";
    };
    recovery = lib.mkOption {
      type = lib.types.str;
      description = "the owner's paper key, as an ssh-ed25519 public key line: what the disk keys' copies in /boot/dd are encrypted to";
    };
    enable = lib.mkEnableOption "making the disks' key in the initrd at every boot (the box enrolled, its pools encrypted)";
    pools = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ "tank" ];
      description = "pools whose import waits for the keys";
    };
  };

  config = lib.mkMerge [
    { environment.systemPackages = [ enrol ]; }
    (lib.mkIf cfg.enable {
      boot.initrd.systemd.enable = true;
      boot.initrd.systemd.tpm2.enable = true;
      # the house cable, as the running box has it (home-network.nix): wifi
      # would want its password, which is on the locked disk
      boot.initrd.availableKernelModules = [
        "r8152"
        "cdc_ether"
        "cdc_ncm"
        "ax88179_178a"
        "xhci_pci"
        "vfat"
        "nls_cp437"
        "nls_iso8859-1"
      ];
      boot.initrd.network.enable = true;
      boot.initrd.network.flushBeforeStage2 = true;
      boot.initrd.systemd.network.networks."10-house" = {
        matchConfig.MACAddress = lib.toLower home.wired;
        address = [ home.address ];
        gateway = [ home.gateway ];
        dns = [
          "1.1.1.1"
          "9.9.9.9"
        ];
      };
      boot.initrd.systemd.initrdBin = tools;
      # the TPM library finds its device driver at run time: carried whole
      boot.initrd.systemd.storePaths = [
        boot
        pkgs.tpm2-tss
      ];
      boot.initrd.systemd.contents."/etc/ssl/certs/ca-certificates.crt".source =
        "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
      boot.initrd.systemd.services.dd-disk-key = {
        description = "Make the disks' key: the TPM's half and the unlock Worker's";
        # the root pool is imported in the initrd and waits for it; a pool
        # imported later (node1's vault) finds the key left in /run/dd
        requiredBy = map (p: "zfs-import-${p}.service") cfg.pools;
        wantedBy = [ "initrd.target" ];
        before = map (p: "zfs-import-${p}.service") cfg.pools ++ [ "initrd.target" ];
        after = [
          "tpm2.target"
          "systemd-networkd.service"
          "systemd-resolved.service"
        ];
        wants = [ "systemd-networkd.service" ];
        unitConfig.DefaultDependencies = false;
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          TimeoutStartSec = "infinity";
          ExecStart = boot;
          StandardOutput = "journal+console";
          Environment = "TPM2TOOLS_TCTI=device:/dev/tpmrm0";
        };
      };
      # the keys leave memory once every pool has them
      systemd.services.dd-disk-key-forget = {
        description = "Forget the disks' key once the pools are open";
        wantedBy = [ "multi-user.target" ];
        after = [
          "zfs-import.target"
          "zfs-mount.service"
        ];
        serviceConfig.Type = "oneshot";
        script = ''
          [ -d /run/dd ] || exit 0
          ${pkgs.coreutils}/bin/shred -u /run/dd/*.key 2>/dev/null || true
          rmdir /run/dd 2>/dev/null || true
        '';
      };
    })
  ];
}
