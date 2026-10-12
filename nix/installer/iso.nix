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
  install = pkgs.writeShellApplication {
    name = "dd-join-install";
    runtimeInputs = [
      self.packages.${sys}.agent
      pkgs.disko
      pkgs.tpm2-tools
      pkgs.age-plugin-tpm
      pkgs.jq
      pkgs.util-linux
      pkgs.coreutils
      pkgs.gawk
      pkgs.gnugrep
      pkgs.findutils
      pkgs.git
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
  environment.systemPackages = [ install ];
  environment.variables = {
    RELEASE_URL = "https://code.${domain}/${repo}/raw/branch/releases/current.json";
    RELEASE_KEY = "/etc/dd/release.pub";
    SOURCE = "git+https://code.${domain}/${repo}";
    LAYOUT = "${../../fleet/layout.nix}";
    DETECT = "${../modules/box/report/detect.sh}";
  };
  # what it says at the console, before anyone types
  services.getty.helpLine = lib.mkForce ''

    This installs a box of the fleet onto this machine's internal disk,
    which is wiped. Run: sudo -E dd-join-install
  '';
}
