{ config, lib, ... }:
let
  boxes = builtins.fromJSON (builtins.readFile ../../fleet/boxes.json);
  me = boxes.${config.networking.hostName} or { };
  policy = lib.importJSON ../../fleet/policy.json;
  # whose box this is, and their keys (fleet/policy.json people)
  owner = policy.people.${config.dd.box.owner};
  # the other boxes, by their address on the fleet's network
  peersNet = lib.mapAttrsToList (_: b: b.net) (
    lib.filterAttrs (n: b: n != config.networking.hostName && b ? net) boxes
  );
  # the other boxes in this house
  housePeersNet = lib.mapAttrsToList (_: b: b.net) (
    lib.filterAttrs (
      n: b: n != config.networking.hostName && b ? net && (b.siteId or null) == (me.siteId or "")
    ) boxes
  );
  # the channels between boxes that are still here, each waiting on the
  # change that ends it (README: no box talks to another box)
  #   3900  garage S3, for a box with no garage of its own (its backups,
  #         the nix cache) - until there is one per box
  #   3901  garage RPC - one cluster, until there is one per box
  # and, between the boxes of one house only:
  #   4181  the house's Wi-Fi relay and page (the directory itself is the
  #         control plane's agreed log now, pulled by nobody)
  boxPorts = "3900,3901";
in
{
  # What every box is, before any role: its identity in the directory, a
  # replica of the directory, its own metrics, and a way in for the admin.
  imports = [
    ./_sops.nix
    ../modules/box/domain.nix
    ../modules/box/box.nix
    ../modules/gate/verify.nix
    # a certificate for the names a box serves, when it serves any
    ../modules/gate/acme.nix
    ../modules/observe/metrics.nix
    # a box's own metrics at metrics.<box>.<domain>, once sign-in and the
    # zone are the Workers'
    ../modules/observe/box-metrics.nix
    ../modules/observe/disks.nix
    ../modules/box/zfs-datasets.nix
    ../modules/storage/usage.nix
    ../modules/storage/view.nix
    ../modules/storage/postgres-isolation.nix
    ../modules/storage/postgres-backup.nix
    ../modules/storage/backup.nix
    ../modules/box/agent.nix
    ../modules/observe/thanos.nix
    ../modules/box/locate.nix
    ../modules/box/netspeed.nix
    ../modules/box/service-memory.nix
    ../modules/net/box.nix
    ../modules/net/owner.nix
  ];

  # the box moves itself to the signed release; the key that signs lives on
  # the laptop that builds, its public half in the repo
  # who may use the services: comes with the release, checked by every box
  dd.members = builtins.fromJSON (builtins.readFile ../../fleet/members.json);
  dd.verify.releasePublicKey = lib.fileContents ../../fleet/release.pub;
  # the mail Worker's read key, once `dd mail reader-key` has made one
  dd.verify.directoryReaders =
    let
      mail = lib.importJSON ../../fleet/mail.json;
    in
    lib.optional (mail ? reader) mail.reader;
  dd.agent = {
    enable = true;
    url = "https://git.${config.dd.domain}/${config.dd.repo}/raw/branch/releases/current.json";
    publicKey = lib.fileContents ../../fleet/release.pub;
    # each box's host key, as dd-attest signs with it: two of them must
    # vouch for a closure before a box switches to it
    attesters = lib.mapAttrs (_: b: b.hostKey) (
      lib.filterAttrs (_: b: b ? hostKey) (lib.importJSON ../../fleet/boxes.json)
    );
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
  dd.domain = policy.domain;
  dd.repo = policy.repo;
  # the box owner's paper key (their entry's recovery key): the box's disk keys
  # have a copy encrypted to it in /boot/dd, for a dead TPM
  dd.unlock.recovery = owner.paperKey;
  # every box holds a directory replica; the gateway role turns this into
  # the full verifier with the browser login
  dd.verify.role = lib.mkDefault "directory";
  dd.verify.admin = policy.owner;

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
    # the box's owner (box.json) logs in as admin
    openssh.authorizedKeys.keys = owner.ssh;
  };

  # Every box is on the fleet's own network, and on no other: the owner's
  # Tailscale account is retired (its login was a GitHub account the
  # fleet does not control). One tailscaled, the fleet's.
  dd.net.enable = true;
  # The network is not trusted wholesale. Trusting an interface opens every
  # port on it to everything that can reach it, which on the fleet's network
  # meant any member's device could talk to postgres, garage and prometheus
  # on every box. What is open is what a person needs: the gate to every
  # member; ssh, dns and the cache push to the box's owner's devices
  # (modules/net/owner.nix); between boxes only the channels listed above,
  # only from the other boxes' addresses. Cable and wifi are untouched: ssh
  # from the LAN is the way in if any of this is ever wrong.
  networking.firewall.interfaces.commonty0.allowedTCPPorts = [ 443 ];
  networking.firewall.extraCommands =
    lib.concatMapStrings (a: ''
      iptables -A nixos-fw -i commonty0 -s ${a} -p tcp -m multiport --dports ${boxPorts} -j nixos-fw-accept
    '') peersNet
    + lib.concatMapStrings (a: ''
      iptables -A nixos-fw -i commonty0 -s ${a} -p tcp --dport 4181 -j nixos-fw-accept
    '') housePeersNet;
  networking.networkmanager.enable = true;
  # never invent a "Wired connection 1": on a first boot NetworkManager can
  # see the cable before the declared profile exists, make a dhcp profile
  # for it, and keep preferring that one. node2 came up on a dhcp address
  # after a reinstall this way; a box has the profiles its hardware file
  # declares and nothing else
  networking.networkmanager.settings.main.no-auto-default = "*";
  # tailscaled owns its tunnel: when NetworkManager restarted mid-release
  # it took the owner's tailscale0 as its own, dropped the address, and the
  # box fell off until tailscaled was restarted (commonty0: modules/net/box.nix).
  # Same for docker's
  networking.networkmanager.unmanaged = [
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

  # The store is cleaned, or it fills the disk: CI builds on a runner box
  # pile up for good otherwise (node2's reached 97G and, with garage on the
  # same pool, took the box's last byte). Weekly, the systems of the last two
  # weeks kept (a rollback is to one of them), and whenever a build finds
  # less than 10G free.
  nix.gc = {
    automatic = true;
    dates = "weekly";
    options = "--delete-older-than 14d";
    randomizedDelaySec = "6h";
  };
  # Builds are held to their share of the box's memory, so a big one is
  # what gets killed, not garage or the gate: node2 (14G) ran out entirely
  # building node1's system while garage resynced. Above 60% builds are
  # slowed, at 75% killed; and one build at a time per 6G, at most three
  # for every four threads. A crate's build is mostly one thread, so one
  # per four threads left node1 compiling two crates at once with six of
  # its eight threads idle (a cold test build took half an hour).
  systemd.services.nix-daemon.serviceConfig =
    let
      mib = config.dd.box.memoryBytes / (1024 * 1024);
    in
    lib.mkIf (config.dd.box.memoryBytes > 0) {
      MemoryHigh = "${toString (mib * 60 / 100)}M";
      MemoryMax = "${toString (mib * 75 / 100)}M";
    };
  nix.settings.max-jobs = lib.mkIf (config.dd.box.memoryBytes > 0) (
    lib.max 1 (
      lib.min (config.dd.box.memoryBytes / (6 * 1024 * 1024 * 1024)) (config.dd.box.threads * 3 / 4)
    )
  );
  nix.settings.min-free = 10 * 1024 * 1024 * 1024;
  nix.settings.max-free = 30 * 1024 * 1024 * 1024;
  nix.settings.experimental-features = [
    "nix-command"
    "flakes"
  ];

  system.stateVersion = "26.05";
}
