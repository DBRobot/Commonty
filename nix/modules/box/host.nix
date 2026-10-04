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
in
{
  imports = [
    (dir + "/hardware-configuration.nix")
    (import ../../../fleet/layout.nix {
      device = hw.bootDisk;
      inherit (box) encrypted;
    })
    ./home-network.nix
  ];

  config = lib.mkMerge [
    {
      dd.box.bulkPool = (box.pools or { }).bulk or "tank";
      dd.box.owner = box.owner or null;
      dd.box.threads = hw.cpu.threads;
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
    {
      dd.zfs.datasets = lib.mapAttrs (_: d: removeAttrs d [ "note" ]) box.datasets;
    }
    (lib.mkIf (box ? arcMaxBytes) {
      boot.extraModprobeConfig = ''
        options zfs zfs_arc_max=${toString box.arcMaxBytes}
      '';
    })
    (lib.mkIf box.neverSleep {
      # no suspend on close, on battery or not, and no idle action: a box
      # that sleeps is a box that is gone
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
    })
    { networking.hostId = hw.hostId; }
    # a usb ethernet adapter must never autosuspend: it is the wired link
    (lib.mkIf hw.usbNic { boot.kernelParams = [ "usbcore.autosuspend=-1" ]; })
  ];
}
