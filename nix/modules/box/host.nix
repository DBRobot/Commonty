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
in
{
  imports = [
    (dir + "/hardware-configuration.nix")
    (import ../../../fleet/layout.nix {
      device = hw.bootDisk;
      inherit (box) encrypted;
    })
    ./home-network.nix
    ../storage/store.nix
    ../storage/postgres-follow.nix
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

      dd.home = {
        inherit (house) ssid address;
        pskFile = config.sops.templates."wifi.env".path;
        inherit (hw) wifi wired;
      };
      sops.secrets.wifi-psk = { };
      sops.templates."wifi.env".content = "psk=${config.sops.placeholder.wifi-psk}\n";
    }
    (lib.optionalAttrs (options.dd ? adblock) {
      # who lives in this house: they switch its ad blocking
      dd.adblock.household = lib.mkIf (house ? household) house.household;
    })
    (lib.mkIf (box.extraPools != [ ]) { boot.zfs.extraPools = box.extraPools; })
    (lib.mkIf (box ? runnerCapacity) { dd.runner.capacity = box.runnerCapacity; })
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
      # is gone - garage's second copy, the front door, a game - until there is
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
      };
    }
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
    # a usb ethernet adapter must never autosuspend: it is the wired link
    (lib.mkIf hw.usbNic { boot.kernelParams = [ "usbcore.autosuspend=-1" ]; })
  ];
}
