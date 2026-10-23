{ config, lib, ... }:
let
  boxes = builtins.fromJSON (builtins.readFile ../../fleet/boxes.json);
  me = boxes.${config.networking.hostName} or { };
  policy = lib.importJSON ../../fleet/policy.json;
  # whose box this is, and their keys (fleet/policy.json people)
  # (nobody's, for a box that joined on its own key: no one's keys log in)
  owner =
    if config.dd.box.owner == null then
      {
        ssh = [ ];
        paperKey = null;
      }
    else
      policy.people.${config.dd.box.owner};
  # the other boxes in this house
  housePeersNet = lib.mapAttrsToList (_: b: b.net) (
    lib.filterAttrs (
      n: b: n != config.networking.hostName && b ? net && (b.siteId or null) == (me.siteId or "")
    ) boxes
  );
  # the one channel between boxes still here, waiting on the change that
  # ends it (README: no box talks to another box), between the boxes of one
  # house only:
  #   4181  the house's Wi-Fi relay and page (the directory itself is the
  #         control plane's agreed log now, pulled by nobody)
  # The stores' port, 3443, is each store's own (storage/store.nix).
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
    # whether the services members use work here, for the uptime page
    ../modules/observe/uptime.nix
    ../modules/box/zfs-datasets.nix
    ../modules/storage/usage.nix
    ../modules/storage/view.nix
    ../modules/storage/postgres-isolation.nix
    ../modules/storage/postgres-backup.nix
    ../modules/storage/backup.nix
    ../modules/storage/snapshots.nix
    ../modules/box/agent.nix
    # and an ssh session coming or going cannot fail its switch
    ../modules/box/logins.nix
    ../modules/observe/thanos.nix
    # a joined box's metrics, to the observe box and its member alone
    ../modules/observe/joined-metrics.nix
    ../modules/box/locate.nix
    ../modules/box/netspeed.nix
    ../modules/box/timer-watch.nix
    ../modules/box/service-memory.nix
    ../modules/box/work-memory.nix
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
    # (from the public door for a box that joined on its own key, which
    # the fleet's network keeps out of the forge)
    url = "https://${
      if config.dd.box.joined then "code" else "git"
    }.${config.dd.domain}/${config.dd.repo}/raw/branch/releases/current.json";
    # and the control plane's relay at Cloudflare, which keeps the newest
    # release any box hands it, signature checked: what a box reads when
    # the forge, on one box, is down (docs/dev/releases.md)
    mirrors = lib.optional (builtins.pathExists ../../fleet/control-worker.json) "https://control.${(lib.importJSON ../../fleet/control-worker.json).domain}/release";
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
  # the datasets the box's pool holds, kept: snapshots and scrubs, on each
  # box of the release's (a box that joined on its own key keeps nothing of
  # its own worth them). Its backup's password is the one secret of its
  # own, shared with nobody, where its file holds it; the repository is in
  # the box's own store, under the store's key to its bucket
  # (storage/backup.nix)
  services.zfs.autoSnapshot.enable = !config.dd.box.joined;
  services.zfs.autoScrub.enable = !config.dd.box.joined;
  sops.secrets.restic-password = lib.mkIf (config.dd.hasSecret "restic-password") { };
  dd.backup.passwordFile = lib.mkIf (config.dd.hasSecret "restic-password") config.sops.secrets.restic-password.path;
  # the metric blocks go to the store that keeps the metrics bucket, its
  # home's, with this box's own key there (observe/thanos.nix)
  dd.thanos.fromStore = true;
  dd.domain = policy.domain;
  dd.repo = policy.repo;
  # the box owner's paper key (their entry's recovery key): the box's disk keys
  # have a copy encrypted to it in /boot/dd, for a dead TPM
  # (none for a box nobody owns, whose disks are not encrypted: what it
  # keeps is ciphertext already)
  dd.unlock.recovery = if owner.paperKey == null then "" else owner.paperKey;
  # every box holds a directory replica; the gateway role turns this into
  # the full verifier with the browser login
  dd.verify.role = lib.mkDefault "directory";
  dd.verify.admin = policy.owner;
  dd.verify.people = lib.unique ([ policy.owner ] ++ builtins.attrNames (policy.people or { }));

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
  sops.secrets.console-password-hash = lib.mkIf (config.dd.hasSecret "console-password-hash") {
    neededForUsers = true;
  };
  # and it has to be false for that hash to reach /etc/shadow: with
  # mutable users nixos writes a declarative password only for a user it
  # is creating, so on these boxes admin kept the `!` it was made with and
  # the keyboard way in did not exist. Nobody has ever set a password or
  # made a user by hand here (checked on both boxes); users are what the
  # repo says they are.
  users.mutableUsers = false;
  # a box that joined on its own key is nobody's: nobody logs in to it, by
  # keyboard or ssh, which is what it should be
  users.allowNoPasswordLogin = config.dd.box.owner == null;

  users.users.admin = {
    isNormalUser = true;
    # none on a box without it: no keyboard way in there
    hashedPasswordFile =
      if config.dd.hasSecret "console-password-hash" then
        config.sops.secrets.console-password-hash.path
      else
        null;
    extraGroups = [ "wheel" ];
    # the fleet's admin (policy.json owner) logs in as admin, on every box
    # of the fleet's; the box's host (box.json owner) has no shell on it - a
    # host is not an admin, and a box holds other people's work. A box that
    # joined on its own key is nobody's (above): nobody logs in to it
    openssh.authorizedKeys.keys = lib.optionals (
      config.dd.box.owner != null
    ) policy.people.${policy.owner}.ssh;
  };

  # Every box is on the fleet's own network, and on no other: the owner's
  # Tailscale account is retired (its login was a GitHub account the
  # fleet does not control). One tailscaled, the fleet's.
  dd.net.enable = true;
  # The network is not trusted wholesale. Trusting an interface opens every
  # port on it to everything that can reach it, which on the fleet's network
  # meant any member's device could talk to postgres, garage and prometheus
  # on every box. What is open is what a person needs: the gate to every
  # member; ssh, dns and the cache push (at its store) to the fleet's
  # admin's devices, dns alone to the box's host's (modules/net/owner.nix);
  # the store's port (storage/store.nix); between boxes only the channel
  # listed above, only from the house's other boxes' addresses. Cable and wifi are untouched: ssh
  # from the LAN is the way in if any of this is ever wrong.
  networking.firewall.interfaces.commonty0.allowedTCPPorts = [ 443 ];
  networking.firewall.extraCommands = lib.concatMapStrings (a: ''
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
  # and with a build order and CI's jobs, under the work slice's limit
  # (box/work-memory.nix); a killed build fails, the daemon goes on
  dd.work.units.nix-daemon.oomPolicy = "continue";
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
