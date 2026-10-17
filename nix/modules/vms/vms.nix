{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.vms;
  dir = "/var/lib/dd-vms";
  # the guest's own nixpkgs, without the box's overlays: the guest rebuilds
  # from plain nixpkgs (guest/base.nix), so the host's first system is the
  # one the guest would build
  guestSystem = import "${pkgs.path}/nixos/lib/eval-config.nix" {
    system = pkgs.stdenv.hostPlatform.system;
    modules = [
      ./guest/base.nix
      ./guest/member.nix
    ];
  };
  # the first disk every computer starts from; each copies it once, and
  # from then on it is the instance's own
  image = import "${pkgs.path}/nixos/lib/make-disk-image.nix" {
    inherit pkgs lib;
    inherit (guestSystem) config;
    name = "dd-vm-image";
    format = "qcow2";
    partitionTableType = "legacy";
    diskSize = "auto";
    additionalSpace = "2G";
    copyChannel = false;
    configFile = pkgs.writeText "configuration.nix" ''
      { imports = [ /etc/dd/base.nix ./member.nix ]; }
    '';
    contents = [
      {
        source = ./guest/member.nix;
        target = "/etc/nixos/member.nix";
      }
    ];
  };
  range = "${toString cfg.portBase}:${toString (cfg.portBase + cfg.portCount - 1)}";

  # One instance, by id, from the host's record: its size, its ports, the
  # owner's name, keys and Nix file. The disks are made the first time and
  # grown when the record grows; never shrunk.
  run = pkgs.writeShellScript "dd-vm-run" ''
    set -eu
    id="$1"
    d=${dir}/instances/"$id"
    rec=${dir}/records/"$id".json
    qmp=${dir}/control/"$id".qmp
    serial=${dir}/control/"$id".serial
    [ -s "$rec" ] || { echo "no such instance: $id" >&2; exit 1; }
    jq() { ${pkgs.jq}/bin/jq "$@"; }
    img() { ${pkgs.qemu-utils}/bin/qemu-img "$@"; }
    mem=$(jq -r '.memory' "$rec")
    threads=$(jq -r '.threads' "$rec")
    disk=$(jq -r '.disk' "$rec")
    fwd=$(jq -r '[{guest: 22, host: .port}] + (.forwards // []) | map("hostfwd=tcp::\(.host)-:\(.guest)") | join(",")' "$rec")
    mkdir -p "$d"/config "$d"/report
    if [ ! -e "$d"/system.qcow2 ]; then
      img convert -O qcow2 ${image}/nixos.qcow2 "$d"/system.qcow2.new
      mv "$d"/system.qcow2.new "$d"/system.qcow2
    fi
    [ -e "$d"/home.qcow2 ] || img create -q -f qcow2 "$d"/home.qcow2 "$disk"G
    for f in system home; do
      have=$(img info --output=json "$d/$f.qcow2" | jq '.["virtual-size"]')
      if [ "$have" -lt $(( disk * 1024 * 1024 * 1024 )) ]; then
        img resize -q "$d/$f.qcow2" "$disk"G
      fi
    done
    # what the guest reads, written from the host's record each start: the
    # share is read-only to the guest
    jq '{name: .owner, keys: (.keys // [])}' "$rec" > "$d"/config/owner.json
    jq -r '.nix // "{ ... }: { }"' "$rec" > "$d"/config/member.nix
    rm -f "$qmp" "$serial"
    if [ -w /dev/kvm ]; then accel="accel=kvm -cpu host"; else accel="accel=tcg -cpu max"; fi
    exec ${pkgs.qemu_kvm}/bin/qemu-system-x86_64 \
      -name vm-"$id" -machine q35,$accel -m "$mem" -smp "$threads" \
      -nodefaults -display none -vga none \
      -drive file="$d"/system.qcow2,if=virtio,serial=system,format=qcow2 \
      -drive file="$d"/home.qcow2,if=virtio,serial=home,format=qcow2 \
      -virtfs local,path="$d"/config,mount_tag=config,security_model=none,readonly=on \
      -virtfs local,path="$d"/report,mount_tag=report,security_model=none \
      -netdev user,id=n0,"$fwd" -device virtio-net-pci,netdev=n0 \
      -device virtio-rng-pci \
      -qmp unix:"$qmp",server=on,wait=off \
      -serial unix:"$serial",server=on,wait=off
  '';
  # as a game's: the guest powers down, then qemu is told to quit
  stop = pkgs.writeShellScript "dd-vm-stop" ''
    qmp=${dir}/control/"$1".qmp
    [ -S "$qmp" ] || exit 0
    printf '%s\n' '{"execute":"qmp_capabilities"}' '{"execute":"system_powerdown"}' \
      | ${pkgs.socat}/bin/socat - UNIX-CONNECT:"$qmp" >/dev/null 2>&1 || true
    for i in $(seq 1 60); do kill -0 "$MAINPID" 2>/dev/null || exit 0; sleep 1; done
    printf '%s\n' '{"execute":"qmp_capabilities"}' '{"execute":"quit"}' \
      | ${pkgs.socat}/bin/socat - UNIX-CONNECT:"$qmp" >/dev/null 2>&1 || true
    for i in $(seq 1 15); do kill -0 "$MAINPID" 2>/dev/null || exit 0; sleep 1; done
  '';
in
{
  imports = [ ../guests/egress.nix ];

  # Members' own computers (docs/dev/instances.md): one VM each, from a
  # record on this box, with a disk for its system and one for /home. As
  # with games, that an instance exists is the box's state, not the repo's,
  # and a deploy never restarts one.
  options.dd.vms = {
    enable = lib.mkEnableOption "members' own computers";
    portBase = lib.mkOption {
      type = lib.types.port;
      default = 27200;
      description = "the first port instances' SSH and forwards are handed (games have 27000 up)";
    };
    portCount = lib.mkOption {
      type = lib.types.ints.positive;
      default = 200;
    };
    image = lib.mkOption {
      type = lib.types.package;
      readOnly = true;
      default = image;
      description = "the disk a new instance's system starts as";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.isx86_64;
        message = "members' computers are x86_64 guests for now";
      }
    ];
    dd.guests.enable = true;
    dd.box.plaintext = [ "members' computers (their disks and memory)" ];

    users.users.dd-vms = {
      uid = 952;
      isSystemUser = true;
      group = "dd-vms";
      extraGroups = [ "kvm" ];
      home = dir;
    };
    users.groups.dd-vms.gid = 952;
    systemd.tmpfiles.rules = [
      "d ${dir} 0750 dd-vms dd-vms -"
      "d ${dir}/instances 0750 dd-vms dd-vms -"
      # what the host goes by, out of every guest's reach
      "d ${dir}/records 0750 dd-vms dd-vms -"
      "d ${dir}/control 0750 dd-vms dd-vms -"
    ];

    systemd.services."dd-vm@" = {
      description = "Member's computer %i";
      after = [ "firewall.service" ];
      wants = [ "firewall.service" ];
      restartIfChanged = false;
      stopIfChanged = false;
      unitConfig.X-StopOnRemoval = false;
      serviceConfig = {
        User = "dd-vms";
        Group = "dd-vms";
        # what it may reach out to (nix/modules/guests/egress.nix)
        SupplementaryGroups = [ config.dd.guests.group ];
        ExecStart = "${run} %i";
        ExecStop = "${stop} %i";
        TimeoutStopSec = 90;
        Restart = "on-failure";
        RestartSec = 20;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [
          "${dir}/instances"
          "${dir}/control"
        ];
        ProtectHome = true;
        PrivateTmp = true;
        DeviceAllow = [ "/dev/kvm rw" ];
      };
    };

    # every instance whose record says up, at boot; the manager does this
    # once there is one
    systemd.services.dd-vms-up = {
      description = "Start members' computers that should be up";
      wantedBy = [ "multi-user.target" ];
      after = [
        "firewall.service"
        "systemd-tmpfiles-setup.service"
      ];
      path = [
        pkgs.jq
        pkgs.systemd
      ];
      serviceConfig.Type = "oneshot";
      script = ''
        for rec in ${dir}/records/*.json; do
          [ -e "$rec" ] || continue
          [ "$(jq -r '.up // false' "$rec")" = true ] || continue
          systemctl start --no-block "dd-vm@$(basename "$rec" .json).service"
        done
      '';
    };

    # SSH and forwards are open on the Commonty network only, to whom the
    # owner lets in; the chain is filled once there is a manager (step 2)
    networking.firewall.extraCommands = ''
      for ipt in iptables ip6tables; do
        $ipt -N dd-vms 2>/dev/null || true
        $ipt -D nixos-fw -i commonty0 -p tcp --dport ${range} -j dd-vms 2>/dev/null || true
        $ipt -I nixos-fw 1 -i commonty0 -p tcp --dport ${range} -j dd-vms
      done
    '';
  };
}
