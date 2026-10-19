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
  utils,
  ddScript,
  ...
}:
let
  cfg = config.dd.unlock;
  box = config.networking.hostName;
  home = config.dd.home;
  tools = [
    pkgs.tpm2-tools
    (import ../../lib/tpm-policy.nix pkgs)
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
      ENROL_WAIT = cfg.waits.enrolment;
      WAIT_WAITING = cfg.waits.waiting;
      WAIT_REFUSED = cfg.waits.refused;
      TPM_ONLY = if cfg.tpmOnly then "1" else "";
    }
  );
  # the pools are this box's own before anything on them runs (disk-check.sh)
  check = pkgs.writeShellScript "dd-disk-check" (builtins.readFile ./disk-check.sh);
  # the pools the initrd imports (root's) and the ones imported after it
  poolOf = d: builtins.head (lib.splitString "/" d);
  early = lib.unique (
    map (fs: poolOf fs.device) (
      lib.filter (fs: fs.fsType == "zfs" && utils.fsNeededForBoot fs) (lib.attrValues config.fileSystems)
    )
  );
  late = lib.filter (p: !(builtins.elem p early)) config.boot.zfs.extraPools;
  # the same, on the running box, as root: the keys in /run/dd again, for
  # making or opening a dataset by hand (shredded at the next boot's end)
  now = pkgs.writeShellApplication {
    name = "dd-unlock-now";
    runtimeInputs = tools;
    text =
      "export TPM2TOOLS_TCTI=device:/dev/tpmrm0\n"
      + ddScript ./disk-unlock-boot.sh {
        BOX = box;
        URL = cfg.url;
        OUT = "/run/dd";
        ENROL_WAIT = 1;
        WAIT_WAITING = cfg.waits.waiting;
        WAIT_REFUSED = cfg.waits.refused;
        TPM_ONLY = if cfg.tpmOnly then "1" else "";
      };
  };
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
    # a box that joined on its own (docs/dev/joining.md): no half at the
    # unlock Worker, which knows nothing of it. The key is the TPM's half
    # alone, so a pulled disk opens nothing and the box boots by itself
    tpmOnly = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "the disks' key from the TPM's half alone, with no network at boot";
    };
    # how long the initrd waits between tries; the test shortens them
    waits = {
      enrolment = lib.mkOption {
        type = lib.types.int;
        default = 30;
        description = "seconds to look for the enrolment on the boot partition before going on without it";
      };
      waiting = lib.mkOption {
        type = lib.types.int;
        default = 30;
        description = "seconds between asks while a member has not let the box in";
      };
      refused = lib.mkOption {
        type = lib.types.int;
        default = 60;
        description = "seconds between asks while the box is marked stolen";
      };
    };
    pools = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ "tank" ];
      description = "pools whose import waits for the keys";
    };
  };

  config = lib.mkMerge [
    {
      environment.systemPackages = [
        enrol
        now
      ];
    }
    (lib.mkIf cfg.enable {
      boot.initrd.systemd.enable = true;
      boot.initrd.systemd.tpm2.enable = true;
      # a boot that stops before the pools open stops there: no shell
      boot.initrd.systemd.emergencyAccess = lib.mkForce false;
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
      boot.initrd.network.enable = !cfg.tpmOnly;
      boot.initrd.network.flushBeforeStage2 = !cfg.tpmOnly;
      boot.initrd.systemd.network.networks."10-house" = lib.mkIf (!cfg.tpmOnly) {
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
        check
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
        ]
        ++ lib.optionals (!cfg.tpmOnly) [
          "systemd-networkd.service"
          "systemd-resolved.service"
        ];
        wants = lib.optional (!cfg.tpmOnly) "systemd-networkd.service";
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
      # Before root is mounted, and before the later pools' datasets are:
      # each pool is the one our key opened, or the keys go and boot stops
      boot.initrd.systemd.services.dd-disk-check = lib.mkIf (early != [ ]) {
        description = "Check the root pool is this box's own before booting it";
        requiredBy = [ "sysroot.mount" ];
        before = [ "sysroot.mount" ];
        after = map (p: "zfs-import-${p}.service") early;
        requires = map (p: "zfs-import-${p}.service") early;
        unitConfig.DefaultDependencies = false;
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = "${check} ${lib.concatStringsSep " " early}";
          StandardOutput = "journal+console";
          StandardError = "journal+console";
          Environment = "TPM2TOOLS_TCTI=device:/dev/tpmrm0";
        };
      };
      systemd.services.dd-disk-check = lib.mkIf (late != [ ]) {
        description = "Check the later pools are this box's own before mounting them";
        requiredBy = [ "zfs-mount.service" ];
        before = [ "zfs-mount.service" ];
        after = map (p: "zfs-import-${p}.service") late;
        unitConfig.DefaultDependencies = false;
        path = tools ++ [ config.boot.zfs.package ];
        environment.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = "${check} ${lib.concatStringsSep " " late}";
        };
      };

      # a box that joined, at its first boot with Secure Boot on: the disk's
      # key sealed under this boot, the installer's half gone
      systemd.services.dd-unlock-seal = lib.mkIf cfg.tpmOnly {
        description = "Seal the disk's key under this box's own signed boot";
        wantedBy = [ "multi-user.target" ];
        after = [
          "zfs-mount.service"
          "local-fs.target"
        ];
        before = [ "dd-disk-key-forget.service" ];
        unitConfig.ConditionPathExists = "/boot/dd/unlock/bootstrap.priv";
        restartIfChanged = false;
        path = tools ++ [ config.boot.zfs.package ];
        environment.POOL = lib.head cfg.pools;
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
        };
        script = builtins.readFile ./disk-unlock-seal.sh;
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
