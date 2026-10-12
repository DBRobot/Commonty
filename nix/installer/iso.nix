# The installer a blank machine boots to become a box of the fleet with
# nothing set up by hand (join-install.sh; docs/dev/joining.md). It holds
# the release key's public half and where the fleet publishes, and nothing
# secret: it can be handed to anyone.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  sys = pkgs.stdenv.hostPlatform.system;
  domain = (lib.importJSON ../../fleet/policy.json).domain;
  repo = (lib.importJSON ../../fleet/policy.json).repo;
  # the layout's wipe-and-make script, built here so the installer needs
  # no nixpkgs to evaluate it: for /dev/dd-target, which it points at the disk
  wipe =
    (pkgs.nixos {
      imports = [
        self.inputs.disko.nixosModules.disko
        # encrypted, under the key the installer makes from the TPM's half
        (import ../../fleet/layout.nix {
          device = "/dev/dd-target";
          encrypted = true;
        })
      ];
      networking.hostId = "dd000000";
      boot.loader.grub.enable = false;
      system.stateVersion = "26.05";
    }).config.system.build.destroyFormatMount;
  install = pkgs.writeShellApplication {
    name = "dd-join-install";
    runtimeInputs = [
      self.packages.${sys}.agent
      pkgs.tpm2-tools
      pkgs.age-plugin-tpm
      pkgs.age
      pkgs.openssl
      pkgs.sbctl
      pkgs.jq
      pkgs.util-linux
      pkgs.coreutils
      pkgs.gawk
      pkgs.gnugrep
      pkgs.findutils
      pkgs.git
      # what the report finds the hardware with (box/report.nix)
      pkgs.bash
      pkgs.libva-utils
      pkgs.pciutils
      pkgs.usbutils
      pkgs.ethtool
      pkgs.iproute2
      pkgs.nvme-cli
      config.system.build.nixos-install
      config.system.build.nixos-generate-config
      config.boot.zfs.package
    ];
    text = builtins.readFile ./join-install.sh;
  };
in
{
  boot.supportedFilesystems.zfs = true;
  networking.hostId = "dd000000";
  nix.settings.experimental-features = [
    "nix-command"
    "flakes"
  ];
  environment.etc."dd/release.pub".source = ../../fleet/release.pub;
  environment.etc."dd/wipe".source = "${wipe}/bin/disko-destroy-format-mount";
  environment.systemPackages = [ install ];
  environment.variables = {
    RELEASE_URL = "https://code.${domain}/${repo}/raw/branch/releases/current.json";
    RELEASE_KEY = "/etc/dd/release.pub";
    SOURCE = "git+https://code.${domain}/${repo}";
    DETECT = "${../modules/box/report/detect.sh}";
    # the commit this installer was made from: no release issued before it
    NOT_BEFORE = toString (self.lastModified or 0);
    # where a disk's backup goes before it is wiped (backup.sh)
    FILES = "https://files.${domain}";
  };
  # what it says at the console, before anyone types
  services.getty.helpLine = lib.mkForce ''

    This installs a box of the fleet onto this machine's internal disk,
    which is wiped. Run: sudo -E dd-join-install
  '';
}
