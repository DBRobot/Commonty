# What every member's computer is before its owner's own Nix file: a NixOS
# machine on a disk of its own, with its own store, that builds its own
# system. The host builds the first disk from this and the release's
# nixpkgs; after that the guest rebuilds itself, from this file (kept in
# /etc/dd) and the owner's (/etc/nixos/member.nix), and the host never
# evaluates or builds what the owner wrote.
#
# From the host it gets two shares: config (read-only: the owner's name,
# their SSH keys, their Nix file, written from the host's record) and
# report (what the guest says back: whether the Nix file applied).
{ lib, pkgs, ... }:
let
  config9p = "/run/dd/config";
  report9p = "/run/dd/report";
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
  fileSystems.${report9p} = share "report" false;

  # this file, by content: the same store path whether the host evaluates it
  # from the repo or the guest from /etc/dd
  environment.etc."dd/base.nix".source = ./base.nix;
  environment.etc."dd/nixpkgs".source = pkgs.path;
  nix.nixPath = [
    "nixpkgs=/etc/dd/nixpkgs"
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
    environment.NIX_PATH = "nixpkgs=/etc/dd/nixpkgs:nixos-config=/etc/nixos/configuration.nix";
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
      tail -c 8192 /var/lib/dd/apply.log > ${report9p}/apply.log || true
      jq -n --arg hash "$want" --argjson ok $ok --argjson at "$(date +%s)" \
        --argjson took $(( $(date +%s) - start )) \
        '{hash: $hash, ok: $ok, at: $at, took: $took}' > ${report9p}/apply.json.new
      mv ${report9p}/apply.json.new ${report9p}/apply.json
    '';
  };
  systemd.timers.dd-apply = {
    wantedBy = [ "timers.target" ];
    timerConfig.OnUnitActiveSec = "30s";
  };
}
