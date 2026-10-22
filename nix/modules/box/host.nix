# One box, from its folder in fleet/boxes/<name>: no per-box Nix.
#   hardware-configuration.nix  what nixos-generate-config found on the box
#   hardware.json               what else the box found: disks, links, GPU
#   box.json                    what its owner chose: house, pools, datasets
# The first two are the box's own report; box.json is the owner's.
dir:
{
  config,
  options,
  lib,
  pkgs,
  ...
}:
let
  hw = lib.importJSON (dir + "/hardware.json");
  box = lib.importJSON (dir + "/box.json");
  house = box.house;
  # its Wi-Fi password, sealed to it by the installer (join-install.sh)
  sealedWifi = house.sealedWifi or false;
  # the other boxes with a store of their own (box.json store), and what
  # their secrets keys are: what is sealed to them
  fleet = builtins.attrNames (lib.importJSON ../../../fleet/boxes.json);
  secretsKey = n: (lib.importJSON (../../../fleet/boxes + "/${n}/hardware.json")).secretsKey or null;
  # every box with a secrets key has a store
  stores = lib.filter (n: secretsKey n != null) fleet;
  # who may hold the postgres lease (the flake's lease rules): the archive's
  # key is sealed to them, and to no other box that keeps a copy of it
  placement = import ../../lib/fleet-placement.nix { inherit lib; };
  pgAble = lib.filter (n: placement.inputs.boxes.${n}.can ? postgresql) stores;
  netAble = lib.filter (n: placement.inputs.boxes.${n}.can ? headscale) stores;
  # whether a box of the release gathers joined boxes' metrics: a joined
  # box sends its own to metrics.<domain> on the store's port, a name that
  # follows the fleet view, and the receive with it (observe/needs.nix)
  gathered = placement.builtWith "joined-metrics" != [ ];
  # the box that serves the members' libraries, the one built with them:
  # the one box that writes them (store.nix writer)
  libraryBox = lib.findFirst (_: true) null (placement.builtWith "libraries");
  # the house's other boxes, by their owners' choices (box/net-hardware.nix):
  # those on its Wi-Fi with a fixed address
  houseBoxes = lib.mapAttrs (_: o: lib.head (lib.splitString "/" o.house.address)) (
    lib.filterAttrs (
      n: o:
      n != config.networking.hostName
      && (o.house or { }).ssid or null == house.ssid
      && (o.house or { }) ? address
    ) (lib.genAttrs fleet (n: lib.importJSON (../../../fleet/boxes + "/${n}/box.json")))
  );
in
{
  imports = [
    (dir + "/hardware-configuration.nix")
    (import ../../../fleet/layout.nix {
      device = hw.bootDisk;
      inherit (box) encrypted;
    })
    ./home-network.nix
    (import ./capability.nix dir)
    ../storage/store.nix
    ../storage/postgres-follow.nix
    ./net-hardware.nix
  ];

  config = lib.mkMerge [
    {
      dd.box.bulkPool = (box.pools or { }).bulk or "tank";
      dd.box.owner = box.owner or null;
      dd.box.threads = hw.cpu.threads;
      dd.box.memoryBytes = hw.memoryBytes;
      # a line its owner has not said is flat counts as metered
      dd.box.metered = (box.internet or { }).metered or true;
      dd.box.disks = map (d: d.id) (hw.disks or [ ]);
      dd.box.gpus = map (g: { inherit (g) render driver encode; }) hw.gpus;
      # its links, and the house's other boxes (box/net-hardware.nix)
      dd.box.links.report = hw;
      dd.box.links.houseBoxes = houseBoxes;
    }
    # Any GPU, whoever made it: Mesa's Vulkan drivers (Intel's, AMD's, and
    # NVK for NVIDIA from the GTX 16 series on) come with graphics on. Video
    # encoding: Intel's needs its media driver; AMD's is in Mesa.
    (lib.mkIf (hw.gpus != [ ]) {
      hardware.graphics.enable = true;
      hardware.graphics.extraPackages = lib.optional (lib.any (
        g:
        builtins.elem g.driver [
          "i915"
          "xe"
        ]
      ) hw.gpus) pkgs.intel-media-driver;
    })
    # the first GPU the report found encoding H.264: transcode works there
    (
      let
        h264 = lib.filter (g: lib.any (lib.hasPrefix "H264") g.encode) hw.gpus;
      in
      lib.mkIf (h264 != [ ]) { dd.box.vaapi = (lib.head h264).render; }
    )
    {
      # the disks' key made in the initrd at every boot, from this box's TPM
      # and the unlock Worker (modules/box/disk-unlock.nix)
      dd.unlock.enable = box.encrypted;
      # one that joined on its own: from its TPM alone (docs/dev/joining.md)
      dd.unlock.tpmOnly = (box.unlock or "worker") == "tpm";

      # Wi-Fi where this box's secrets hold the house's password, or where
      # its installer sealed it to the box (a box that joined: below); wired
      # alone where neither
      dd.home = {
        inherit (house) ssid address;
        pskFile =
          if config.dd.hasSecret "wifi-psk" then
            config.sops.templates."wifi.env".path
          else if sealedWifi then
            "/run/dd-wifi-key/wifi.env"
          else
            "/dev/null";
        inherit (hw) wired;
        wifi = if config.dd.hasSecret "wifi-psk" || sealedWifi then hw.wifi else null;
      };
      sops.secrets.wifi-psk = lib.mkIf (config.dd.hasSecret "wifi-psk") { };
      # the installer's: the password sealed to this box's secrets key, which
      # only its TPM opens; opened into memory before NetworkManager reads it
      systemd.services.dd-wifi-unseal = lib.mkIf sealedWifi {
        description = "Open the Wi-Fi password the installer sealed to this box";
        wantedBy = [ "multi-user.target" ];
        before = [ "NetworkManager-ensure-profiles.service" ];
        requiredBy = [ "NetworkManager-ensure-profiles.service" ];
        unitConfig.ConditionPathExists = "/var/lib/dd/wifi.age";
        path = [
          pkgs.age
          pkgs.age-plugin-tpm
        ];
        environment.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          RuntimeDirectory = "dd-wifi-key";
          RuntimeDirectoryMode = "0700";
          RuntimeDirectoryPreserve = true;
          UMask = "0077";
          ExecStart = "${pkgs.age}/bin/age -d -i /var/lib/dd/secrets-key.txt -o /run/dd-wifi-key/wifi.env /var/lib/dd/wifi.age";
        };
      };
      sops.templates."wifi.env" = lib.mkIf (config.dd.hasSecret "wifi-psk") {
        content = "psk=${config.sops.placeholder.wifi-psk}\n";
      };
    }
    (lib.optionalAttrs (options.dd ? adblock) {
      # who lives in this house: they switch its ad blocking
      dd.adblock.household = lib.mkIf (house ? household) house.household;
    })
    (lib.mkIf (box.extraPools != [ ]) { boot.zfs.extraPools = box.extraPools; })
    # the memory game servers may have together, where a third of the box's
    # is not the owner's choice
    (lib.optionalAttrs (options.dd ? games && box ? gamesMemoryMiB) {
      dd.games.memoryMiB = box.gamesMemoryMiB;
    })
    {
      dd.zfs.datasets = lib.mapAttrs (_: d: removeAttrs d [ "note" ]) box.datasets;
    }
    (lib.mkIf (box ? arcMaxBytes) {
      boot.extraModprobeConfig = ''
        options zfs zfs_arc_max=${toString box.arcMaxBytes}
      '';
    })
    {
      # No box sleeps, whatever it is (David, 2026-10-11): no suspend on close,
      # on battery or not, and no idle action. A box that sleeps is a box that
      # is gone - a store's copies, the front door, a game - until there is
      # a sleep that hands its work on first
      services.logind.settings.Login = {
        HandleLidSwitchDocked = "ignore";
        HandlePowerKey = "ignore";
        IdleAction = "ignore";
      };
      systemd.sleep.settings.Sleep = {
        AllowSuspend = "no";
        AllowHibernation = "no";
        AllowHybridSleep = "no";
        AllowSuspendThenHibernate = "no";
      };
    }
    { networking.hostId = hw.hostId; }
    # its own store, on every box: a dataset of its pool, as big as the pool
    # allows (store.nix capacity auto); box.json store says otherwise. The
    # copies go to the other boxes, each with its own
    {
      dd.zfs.datasets."${config.dd.box.bulkPool}/enc/store" = {
        mountpoint = (box.store or { }).dir or "/srv/store";
        recordsize = "1M";
      };
      # the other boxes reach it at store.<box>.<domain>: nginx and a
      # certificate for this box's own names (gate/acme.nix)
      dd.acme.enable = true;
      dd.store = {
        enable = true;
        capacity = (box.store or { }).capacity or "auto";
        dataset = "${config.dd.box.bulkPool}/enc/store";
        dataDir = (box.store or { }).dir or "/srv/store";
        peers = lib.genAttrs (lib.remove config.networking.hostName stores) (n: {
          url = "https://store.${n}.${config.dd.domain}:${toString config.dd.store.publicPort}";
          recipient = secretsKey n;
        });
        # a bucket for each of them where each writes its own (backups-<box>)
        boxes = stores;
      };
    }
    # the agent fetches each release's closure from the nix cache, in the
    # store that keeps it, with this box's key there
    (lib.mkIf (!config.dd.box.joined && placement.homes ? nix-cache) {
      dd.agent.cache = config.dd.store.at.nix-cache.s3Url;
      dd.agent.cacheEnvFile = config.dd.store.at.nix-cache.envFile;
    })
    # where the shared buckets are kept (placement's homes: the first store
    # able to run each one's service, pinned there in the plan) and who
    # writes the libraries: boxes of the release's, which a joined box's
    # store never keeps. The mail follows its placed service
    (lib.mkIf (!config.dd.box.joined) {
      dd.store.buckets =
        lib.mapAttrs (_: h: { home = h; }) placement.homes
        // lib.optionalAttrs (libraryBox != null) { libraries.writer = libraryBox; };
    })
    # a box that joined on its own key sends its metrics to whichever box
    # the plan gives joined-metrics: metrics.<domain> follows it
    (lib.mkIf (config.dd.box.joined && gathered) {
      dd.joinedMetrics.to = "https://metrics.${config.dd.domain}:${toString config.dd.store.publicPort}";
    })
    # with failover on, postgres follows its lease through this store
    (lib.mkIf (config.dd.failover && config.services.postgresql.enable) {
      dd.store.buckets.postgres-wal.owner = "postgres";
      dd.postgres.follow = {
        enable = true;
        endpoint = "http://127.0.0.1:${toString config.dd.store.port}";
        envFile = "/var/lib/dd-store/keys/postgres-wal.env";
        recipients = lib.genAttrs pgAble secretsKey;
      };
    })
    (lib.optionalAttrs (options.dd ? headscale) {
      dd.headscale.follow.recipients = lib.mkIf config.dd.failover (lib.genAttrs netAble secretsKey);
    })
    # No USB power saving on any box (David, 2026-10-11): an adapter, a disk
    # or a dongle that sleeps is one that may not wake (node2's wired link).
    # No autosuspend, and no USB3 link power states where the kernel offers
    # them; a device whose driver turns autosuspend on is turned back
    {
      boot.kernelParams = [ "usbcore.autosuspend=-1" ];
      services.udev.extraRules = ''
        ACTION=="add|bind", SUBSYSTEM=="usb", TEST=="power/control", ATTR{power/control}="on"
        ACTION=="add|bind", SUBSYSTEM=="usb", TEST=="power/usb3_hardware_lpm_u1", ATTR{power/usb3_hardware_lpm_u1}="0"
        ACTION=="add|bind", SUBSYSTEM=="usb", TEST=="power/usb3_hardware_lpm_u2", ATTR{power/usb3_hardware_lpm_u2}="0"
      '';
    }
  ];
}
