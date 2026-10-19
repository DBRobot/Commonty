# What every member's computer is before its owner's own Nix file: a NixOS
# machine on a disk of its own, with its own store, that builds its own
# system. The host builds the first disk from this and the release's
# nixpkgs; after that the guest rebuilds itself, from this file (by its
# store path, which /etc/nixos/configuration.nix names and dd.guest.base
# keeps) and the owner's (/etc/nixos/member.nix), and the host never
# evaluates or builds what the owner wrote.
#
# From the host it gets a share, config (read-only: the owner's name, the
# SSH public keys their signed record lets in, their Nix file, written from
# the agreed record), and a small
# disk, report, for what it says back: whether the Nix file applied.
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:
let
  config9p = "/run/dd/config";
  share = tag: ro: {
    device = tag;
    fsType = "9p";
    options = [
      "trans=virtio"
      "version=9p2000.L"
      "msize=262144"
      "nofail"
      "x-systemd.device-timeout=10s"
    ]
    ++ lib.optional ro "ro";
  };
in
{
  # virtio's disk, network and 9p drivers in the initrd: without them it
  # waits for /dev/disk/by-label/nixos and gives up
  imports = [
    "${modulesPath}/profiles/qemu-guest.nix"
    {
      options.dd.guest.base = lib.mkOption {
        type = lib.types.path;
        description = "this file's store path, as configuration.nix imports it";
      };
      options.dd.guest.nixpkgs = lib.mkOption {
        type = lib.types.path;
        description = "the nixpkgs the guest rebuilds from: the release's, by its store path";
      };
      options.dd.guest.netbridge = lib.mkOption {
        type = lib.types.path;
        description = "the bridge to the fleet's control server through the front door, from the release, by its store path";
      };
    }
  ];

  system.stateVersion = "26.05";

  boot.loader.grub.device = "/dev/vda";
  boot.growPartition = true;
  boot.kernelParams = [ "console=ttyS0,115200" ];
  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
    autoResize = true;
  };
  # the owner's files: a disk of its own, which a reset of the system leaves
  fileSystems."/home" = {
    device = "/dev/disk/by-id/virtio-home";
    fsType = "ext4";
    autoFormat = true;
    autoResize = true;
  };
  fileSystems.${config9p} = share "config" true;

  # this file, as configuration.nix names it: kept in the guest's store, so
  # a rebuild there finds it. Not ./base.nix: read from the store, a path
  # beside it is /nix/store itself
  # by store path, not through a link in /etc: a link read as nixpkgs is
  # copied into the store under another name, and the system differs
  system.extraDependencies = [
    "${config.dd.guest.base}"
    "${config.dd.guest.nixpkgs}"
    "${config.dd.guest.netbridge}"
  ];
  nix.nixPath = [
    "nixpkgs=${config.dd.guest.nixpkgs}"
    "nixos-config=/etc/nixos/configuration.nix"
  ];
  nix.channel.enable = false;
  nix.settings = {
    experimental-features = [
      "nix-command"
      "flakes"
    ];
    trusted-users = [ "@wheel" ];
  };

  networking.hostName = "commonty";
  networking.useDHCP = true;
  # qemu forwards only the ports the owner's config names; this is the
  # owner's to change
  networking.firewall.allowedTCPPorts = [ 22 ];

  services.openssh = {
    enable = true;
    settings = {
      PasswordAuthentication = false;
      KbdInteractiveAuthentication = false;
      PermitRootLogin = "no";
      # every connection comes from QEMU's user network (10.0.2.2): a few
      # wrong keys from anyone would shut its owner out too
      PerSourcePenalties = "no";
    };
    # copied out of the share by dd-owner: sshd wants keys root owns
    authorizedKeysFiles = [ "/run/dd/keys/%u" ];
  };

  # the owner is made at boot, from the share, not in Nix: their name and
  # keys need no rebuild, and the disk the host made works for anyone
  users.mutableUsers = true;
  security.sudo.wheelNeedsPassword = false;
  # the serial console is the owner's way in when SSH isn't; the host lets
  # only the owner reach it
  services.getty.autologinUser = "root";

  # The Commonty network, for a computer kept to its owner's own boxes: the
  # box that runs it hands a single-use key into the share (net.json) as it
  # starts it. Tagged a computer, it reaches the boxes as the owner's
  # devices do and none of their admin ports. Through the front door, as a
  # box's tailscaled is (netbridge).
  services.tailscale.enable = true;
  # its node key in memory only: it never travels with the disk, and a
  # computer started again joins with the key its box hands in then
  services.tailscale.extraDaemonFlags = [ "--state=mem:" ];
  systemd.services.dd-netbridge = {
    description = "The fleet's control server, through the front door";
    after = [ "run-dd-config.mount" ];
    wantedBy = [ "multi-user.target" ];
    unitConfig.ConditionPathExists = "${config9p}/net.json";
    path = [ pkgs.jq ];
    script = ''
      url=$(jq -r .control_url ${config9p}/net.json)
      case "$url" in https://*) ;; *) echo "not a control server: $url"; exit 1 ;; esac
      exec ${config.dd.guest.netbridge}/bin/netbridge -listen 127.0.0.1:41643 -control "$url"
    '';
    serviceConfig = {
      DynamicUser = true;
      Restart = "always";
      RestartSec = 5;
    };
  };
  systemd.services.dd-net = {
    description = "Join the Commonty network with the key the box handed in";
    after = [
      "tailscaled.service"
      "dd-netbridge.service"
    ];
    wants = [ "dd-netbridge.service" ];
    wantedBy = [ "multi-user.target" ];
    unitConfig.ConditionPathExists = "${config9p}/net.json";
    path = [
      pkgs.jq
      pkgs.tailscale
      pkgs.coreutils
    ];
    serviceConfig.Type = "oneshot";
    script = ''
      if tailscale status --json 2>/dev/null | jq -e '.BackendState == "Running"' >/dev/null; then
        exit 0
      fi
      key=$(mktemp)
      trap 'rm -f "$key"' EXIT
      jq -r .key ${config9p}/net.json > "$key"
      name=$(jq -r .hostname ${config9p}/net.json)
      case "$name" in computer-*) ;; *) exit 1 ;; esac
      tailscale up --login-server=http://127.0.0.1:41643 --auth-key=file:"$key" \
        --hostname="$name" --accept-dns=false --accept-routes=false --timeout=60s
    '';
  };
  systemd.timers.dd-net = {
    wantedBy = [ "timers.target" ];
    timerConfig.OnUnitActiveSec = "2min";
  };

  environment.systemPackages = with pkgs; [
    git
    curl
    vim
    htop
    jq
  ];

  systemd.services.dd-owner = {
    description = "The owner's account and SSH keys, from the host";
    wantedBy = [ "multi-user.target" ];
    before = [ "sshd.service" ];
    after = [
      "home.mount"
      "run-dd-config.mount"
    ];
    path = [
      pkgs.jq
      pkgs.shadow
      pkgs.coreutils
    ];
    serviceConfig.Type = "oneshot";
    script = ''
      f=${config9p}/owner.json
      [ -r "$f" ] || { echo "no owner from the host"; exit 0; }
      name=$(jq -r .name "$f")
      case "$name" in
        "" | *[!a-z0-9_-]*) echo "not a name: $name"; exit 1 ;;
      esac
      id "$name" >/dev/null 2>&1 || useradd -m -G wheel -s /run/current-system/sw/bin/bash "$name"
      install -d -m 0755 /run/dd/keys
      jq -r '.keys[]' "$f" > /run/dd/keys/.new
      chmod 0644 /run/dd/keys/.new
      mv /run/dd/keys/.new /run/dd/keys/"$name"
    '';
  };
  systemd.timers.dd-owner = {
    wantedBy = [ "timers.target" ];
    timerConfig.OnUnitActiveSec = "30s";
  };

  # the owner's Nix file, applied when it changes: copied in, so the system
  # rebuilds the same without the host, then nixos-rebuild. What came of it
  # goes back to the host; a file that fails is not tried again until it
  # changes
  systemd.services.dd-apply = {
    description = "The owner's Nix file, applied";
    after = [
      "network-online.target"
      "run-dd-config.mount"
      "dd-owner.service"
    ];
    wants = [ "network-online.target" ];
    wantedBy = [ "multi-user.target" ];
    # it runs the switch that would restart it
    restartIfChanged = false;
    path = [
      pkgs.jq
      pkgs.coreutils
      pkgs.nixos-rebuild
      pkgs.nix
      pkgs.git
    ];
    environment.NIX_PATH = "nixpkgs=${config.dd.guest.nixpkgs}:nixos-config=/etc/nixos/configuration.nix";
    serviceConfig.Type = "oneshot";
    script = ''
      f=${config9p}/member.nix
      [ -r "$f" ] || exit 0
      mkdir -p /var/lib/dd
      want=$(sha256sum < "$f" | cut -d' ' -f1)
      [ "$want" = "$(cat /var/lib/dd/tried 2>/dev/null)" ] && exit 0
      echo "$want" > /var/lib/dd/tried
      install -m 0644 "$f" /etc/nixos/member.nix
      start=$(date +%s)
      if nixos-rebuild switch > /var/lib/dd/apply.log 2>&1; then ok=true; else ok=false; fi
      # onto the report disk: block 0 what came of it, then the log's end
      dev=/dev/disk/by-id/virtio-report
      [ -b "$dev" ] || exit 0
      r=$(mktemp)
      head -c 12288 /dev/zero > "$r"
      jq -cn --arg hash "$want" --argjson ok $ok --argjson at "$(date +%s)" \
        --argjson took $(( $(date +%s) - start )) \
        '{hash: $hash, ok: $ok, at: $at, took: $took}' | head -c 4095 \
        | dd of="$r" conv=notrunc status=none
      tail -c 8192 /var/lib/dd/apply.log | dd of="$r" bs=4096 seek=1 conv=notrunc status=none
      dd if="$r" of="$dev" bs=4096 count=3 conv=notrunc,fsync status=none
      rm -f "$r"
    '';
  };
  systemd.timers.dd-apply = {
    wantedBy = [ "timers.target" ];
    timerConfig.OnUnitActiveSec = "30s";
  };
}
