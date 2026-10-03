{ config, lib, ... }:
let
  # the other boxes, by the address they reach this one from
  peers = lib.mapAttrsToList (_: b: b.tailnet) (
    lib.filterAttrs (n: _: n != config.networking.hostName) (
      builtins.fromJSON (builtins.readFile ../../fleet/boxes.json)
    )
  );
  # the channels between boxes that are still here, each waiting on the
  # change that ends it (README: no box talks to another box)
  #   3901  garage RPC - one cluster, until there is one per box
  #   4181  a box's directory, pulled by the others
  #  10901  the thanos sidecar, read by the fleet view
  boxPorts = "3901,4181,10901";
in
{
  # What every box is, before any role: its identity in the directory, a
  # replica of the directory, its own metrics, and a way in for the admin.
  imports = [
    ./_sops.nix
    ../modules/box/domain.nix
    ../modules/box/box.nix
    ../modules/gate/verify.nix
    ../modules/observe/metrics.nix
    ../modules/box/zfs-datasets.nix
    ../modules/storage/backup.nix
    ../modules/box/agent.nix
    ../modules/observe/thanos.nix
    ../modules/box/locate.nix
    ../modules/net/box.nix
  ];

  # the box moves itself to the signed release; the key that signs lives on
  # the laptop that builds, its public half in the repo
  # who may use the services: comes with the release, checked by every box
  dd.members = builtins.fromJSON (builtins.readFile ../../fleet/members.json);
  dd.verify.releasePublicKey = lib.fileContents ../../fleet/release.pub;
  dd.agent = {
    enable = true;
    url = "https://git.${config.dd.domain}/${config.dd.repo}/raw/branch/releases/current.json";
    publicKey = lib.fileContents ../../fleet/release.pub;
  };

  # prometheus history cannot be backfilled, so it is in the box's backup
  # The verifier's state, and the measurements. The first is the only copy
  # of every member's sealed library keys and of the directory this box
  # holds; its only protection until now was replication, which is not a
  # backup - it copies a deletion as faithfully as anything else. That
  # metrics history was backed up and this was not is the wrong way round.
  dd.backup.paths = [
    "/var/lib/dd-verify"
    "/var/lib/prometheus2"
  ];
  # the write-ahead log of a running prometheus does not restore (segments
  # out of order); the blocks do, at the cost of the last two hours
  dd.backup.exclude = [ "/var/lib/prometheus2/data/wal" ];
  dd.domain = "commonty.org";
  dd.repo = "david/commonty";
  # every box holds a directory replica; the gateway role turns this into
  # the full verifier with the browser login
  dd.verify.role = lib.mkDefault "directory";

  # The logs name members, their addresses and where they connected from:
  # two weeks is enough to look into a problem, and no more is kept
  services.journald.extraConfig = ''
    MaxRetentionSec=14day
    SystemMaxUse=1G
  '';

  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;
  boot.supportedFilesystems = [ "zfs" ];
  # The pool that holds root was created by the installer, under the
  # installer's host id. The first boot of a template box would refuse it
  # ("previously in use from another system") and stop in the initrd; with
  # no other system on the disk that refusal protects nothing. Force it.
  boot.zfs.forceImportRoot = true;

  time.timeZone = "America/New_York";

  # A way in at the keyboard when the network is gone. Until now these
  # boxes had ssh and nothing else, so a box that lost its network could
  # only be rebooted blind. The passphrase is in secrets/fleet.yaml on the
  # laptop (`dd secret run -- decrypt --extract '["console-password"]'
  # secrets/fleet.yaml`); the box holds only its hash.
  sops.secrets.console-password-hash.neededForUsers = true;
  # and it has to be false for that hash to reach /etc/shadow: with
  # mutable users nixos writes a declarative password only for a user it
  # is creating, so on these boxes admin kept the `!` it was made with and
  # the keyboard way in did not exist. Nobody has ever set a password or
  # made a user by hand here (checked on both boxes); users are what the
  # repo says they are.
  users.mutableUsers = false;

  users.users.admin = {
    isNormalUser = true;
    hashedPasswordFile = config.sops.secrets.console-password-hash.path;
    extraGroups = [ "wheel" ];
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKOR0kI8YSFB9JwqTJJMB+h4EJCSscpdnnGGGaBNRqXj david-bascom@david-bascom-Legion-Pro-5-16ADR10"
    ];
  };

  services.tailscale = {
    enable = true;
    openFirewall = true;
  };
  # Every box is on the fleet's own network. Two tailscaleds on one box
  # share routing table 52 and the same policy rules, and the second one
  # rewrites that table every time it reconfigures: on node2 a keeper that
  # could not join looped all night doing exactly that and took the box
  # off the network while every link stayed up. What made that harmful was
  # the retry, not the coexistence - the control box has run both for days
  # - so the keeper backs off now instead of reconfiguring every thirty
  # seconds, and a release that costs a box the fleet rolls itself back
  # (box/release: the agent's reach check). The owner's tailscale stays
  # until every address has moved to this network; then it is the one
  # tailscaled a box runs, not a second.
  dd.net.enable = true;
  # Neither tunnel is trusted wholesale. Trusting an interface opens every
  # port on it to everything that can reach it, which on the fleet's network
  # meant any member's device could talk to postgres, garage and prometheus
  # on every box. What is open is what a person needs:
  #
  #   the owner's tailnet  ssh, the gate, and dns for the fleet's names; the
  #                        s3 port too, because a release is pushed to the
  #                        cache from the laptop (it wants a key regardless)
  #   the fleet's network  the gate, and nothing else
  #
  # and between boxes only the channels still listed above, only from the
  # other boxes' addresses. Cable and wifi are untouched: ssh from the LAN
  # is the way in if any of this is ever wrong.
  networking.firewall.interfaces.tailscale0 = {
    allowedTCPPorts = [
      22
      53
      80
      443
      3900
    ];
    allowedUDPPorts = [ 53 ];
  };
  networking.firewall.interfaces.commonty0.allowedTCPPorts = [ 443 ];
  networking.firewall.extraCommands = lib.concatMapStrings (a: ''
    iptables -A nixos-fw -i tailscale0 -s ${a} -p tcp -m multiport --dports ${boxPorts} -j nixos-fw-accept
  '') peers;
  networking.networkmanager.enable = true;
  # never invent a "Wired connection 1": on a first boot NetworkManager can
  # see the cable before the declared profile exists, make a dhcp profile
  # for it, and keep preferring that one. node2 came up on a dhcp address
  # after a reinstall this way; a box has the profiles its hardware file
  # declares and nothing else
  networking.networkmanager.settings.main.no-auto-default = "*";
  # tailscaled owns its tunnel: when NetworkManager restarted mid-release
  # it took tailscale0 as its own, dropped the address, and the box fell
  # off the tailnet until tailscaled was restarted. Same for docker's
  networking.networkmanager.unmanaged = [
    "interface-name:tailscale0"
    "interface-name:docker0"
    "interface-name:veth*"
  ];

  services.openssh = {
    enable = true;
    settings.PasswordAuthentication = false;
  };
  security.sudo.wheelNeedsPassword = false;

  # a laptop being a server: the lid is furniture
  services.logind.settings.Login.HandleLidSwitch = "ignore";
  services.logind.settings.Login.HandleLidSwitchExternalPower = "ignore";

  nix.settings.experimental-features = [
    "nix-command"
    "flakes"
  ];

  system.stateVersion = "26.05";
}
