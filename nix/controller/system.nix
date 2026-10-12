# What both of the controller's systems share: a Raspberry Pi 4 or 5, wired
# to the house router, with nothing listening but the door. No sshd, no
# login: the household reaches it through the door, and nothing else
# reaches it at all.
{
  config,
  lib,
  pkgs,
  self,
  modulesPath,
  ...
}:
let
  policy = lib.importJSON ../../fleet/policy.json;
in
{
  imports = [
    ../modules/box/domain.nix
    ../modules/box/agent.nix
    ../modules/net/box.nix
    ../modules/smarthome/home-assistant.nix
    ../modules/smarthome/door.nix
    (modulesPath + "/profiles/minimal.nix")
  ];

  options.dd.controller = {
    installs = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      description = "the seed's: the locked system first-boot installs";
    };
  };

  config = {
    networking.hostName = "home-controller";
    system.stateVersion = "26.05";
    dd.domain = policy.domain;
    dd.repo = policy.repo;

    boot.loader.grub.enable = false;
    boot.loader.generic-extlinux-compatible.enable = true;
    # the mainline kernel runs the Pi 4 and 5 (u-boot and the Pi's firmware
    # are the image's); it is the one cache.nixos.org has built
    boot.kernelPackages = pkgs.linuxPackages;
    boot.supportedFilesystems.zfs = lib.mkForce false;
    boot.initrd.supportedFilesystems.zfs = lib.mkForce false;
    hardware.enableRedistributableFirmware = true;
    # the card wears out under writes: logs in memory, history written
    # every 30 s (home-assistant.nix)
    services.journald.storage = "volatile";
    services.journald.extraConfig = ''
      RuntimeMaxUse=64M
      MaxRetentionSec=14day
    '';
    zramSwap.enable = true;

    # wired, by DHCP from the house router; mDNS so the installer finds it
    networking.useNetworkd = true;
    systemd.network.networks."10-wired" = {
      matchConfig.Name = "en* eth*";
      networkConfig = {
        DHCP = "yes";
        MulticastDNS = true;
      };
    };
    networking.wireless.enable = false;
    services.resolved.enable = true;

    services.openssh.enable = false;
    users.mutableUsers = false;
    users.users.root.hashedPassword = "!";
    # nobody logs in to it, by design: the door is the only way in
    users.allowNoPasswordLogin = true;

    # the Commonty network, with the key the door was handed at pairing
    dd.net = {
      enable = true;
      getKey = ''
        if [ -s /var/lib/commonty-door/network-key ]; then
          cp /var/lib/commonty-door/network-key "$key"
        else
          false
        fi
      '';
    };
    # nothing on it answers but the door: 443 on commonty0, and the pairing
    # page on the house network until it is paired (door.nix)
    networking.firewall = {
      enable = true;
      allowedTCPPorts = [ ];
      interfaces.commonty0.allowedTCPPorts = [ 443 ];
    };

    dd.smarthome.enable = true;
    dd.door.enable = true;

    # releases, like a box: the locked system in the signed release, its
    # closure through the gate's read-only cache door; the door is what
    # must answer after a switch, and there is no sshd to ask for
    dd.agent = {
      enable = true;
      url = "https://git.${config.dd.domain}/${config.dd.repo}/raw/branch/releases/current.json";
      publicKey = lib.fileContents ../../fleet/release.pub;
      cache = "https://cache.${config.dd.domain}";
      verifyPort = config.dd.door.port;
      ssh = false;
      # as on a box: it moves only to a closure two of the fleet's boxes
      # vouched for, each by its ssh host key
      attesters = lib.mapAttrs (_: b: b.hostKey) (
        lib.filterAttrs (_: b: b ? hostKey) (lib.importJSON ../../fleet/boxes.json)
      );
    };

    nix.settings = {
      substituters = lib.mkForce [ ];
      experimental-features = [
        "nix-command"
        "flakes"
      ];
    };
    documentation.enable = false;
  };
}
