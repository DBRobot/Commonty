{
  config,
  ddScript,
  pkgs,
  lib,
  self,
  ...
}:
let
  cfg = config.dd.computers;
  base = config.dd.domain;
  dir = "/var/lib/dd-computers";
  sock = "/run/dd-computers/dd-computers.sock";
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
    name = "dd-computer-image";
    # compressed: copied once per computer, and through the build cache
    format = "qcow2-compressed";
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
  run = pkgs.writeShellScript "dd-computer-run" ''
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
    mkdir -p "$d"/config
    # what the guest says back: a raw disk of fixed size, not a share it
    # could fill (servers/computers, read_report)
    [ -e "$d"/report.img ] || ${pkgs.coreutils}/bin/truncate -s 1M "$d"/report.img
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
    jq '{name: .owner, keys: ([.key // empty] + (.keys // []) | map(select(. != "")))}' "$rec" > "$d"/config/owner.json
    # an empty Nix file is no module: the empty one instead (as write_share)
    jq -r 'if ((.nix // "") | test("^\\s*$")) then "{ ... }: { }" else .nix end' "$rec" > "$d"/config/member.nix
    rm -f "$qmp" "$serial"
    if [ -w /dev/kvm ]; then accel="accel=kvm -cpu host"; else accel="accel=tcg -cpu max"; fi
    exec ${pkgs.qemu_kvm}/bin/qemu-system-x86_64 \
      -name vm-"$id" -machine q35,$accel -m "$mem" -smp "$threads" \
      -nodefaults -display none -vga none \
      -drive file="$d"/system.qcow2,if=none,id=system,format=qcow2 \
      -device virtio-blk-pci,drive=system,serial=system,bootindex=0 \
      -drive file="$d"/home.qcow2,if=none,id=home,format=qcow2 \
      -device virtio-blk-pci,drive=home,serial=home \
      -virtfs local,path="$d"/config,mount_tag=config,security_model=none,readonly=on \
      -drive file="$d"/report.img,if=none,id=report,format=raw \
      -device virtio-blk-pci,drive=report,serial=report \
      -netdev user,id=n0,"$fwd" -device virtio-net-pci,netdev=n0 \
      -device virtio-rng-pci \
      -qmp unix:"$qmp",server=on,wait=off \
      -chardev socket,id=console,path="$serial",server=on,wait=off,logfile="$d"/console.log,logappend=off \
      -serial chardev:console
  '';
  # as a game's: the guest powers down, then qemu is told to quit
  stop = pkgs.writeShellScript "dd-computer-stop" ''
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
  options.dd.computers = {
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
    manager = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "the manager and its page (compute.<domain>); a test of the guest alone turns it off";
    };
    memoryMiB = lib.mkOption {
      type = lib.types.ints.positive;
      default = 16384;
      description = "what one member's computers, together, may take (fleet/policy.json vms, for now)";
    };
    threads = lib.mkOption {
      type = lib.types.ints.positive;
      default = 8;
    };
    reserveMiB = lib.mkOption {
      type = lib.types.ints.unsigned;
      default = 2048;
      description = "memory the box keeps for itself and its services, never given to a computer";
    };
    reach = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      readOnly = true;
      default = [ "${toString cfg.portBase}-${toString (cfg.portBase + cfg.portCount - 1)}" ];
      description = "the ports a member's device may reach on this box, as headscale's policy writes them";
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

    users.users.dd-computers = {
      uid = 952;
      isSystemUser = true;
      group = "dd-computers";
      extraGroups = [ "kvm" ];
      home = dir;
    };
    users.groups.dd-computers.gid = 952;
    systemd.tmpfiles.rules = [
      "d ${dir} 0750 dd-computers dd-computers -"
      "d ${dir}/instances 0750 dd-computers dd-computers -"
      # what the host goes by, out of every guest's reach
      "d ${dir}/records 0750 dd-computers dd-computers -"
      "d ${dir}/control 0750 dd-computers dd-computers -"
    ];

    systemd.services."dd-computer@" = {
      description = "Member's computer %i";
      after = [ "firewall.service" ];
      wants = [ "firewall.service" ];
      restartIfChanged = false;
      stopIfChanged = false;
      unitConfig.X-StopOnRemoval = false;
      serviceConfig = {
        User = "dd-computers";
        Group = "dd-computers";
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

    # The manager: configs, records, and the units they run as. Like the
    # games manager it believes x-dd-user, so it listens on a socket only
    # nginx's group opens
    systemd.services.dd-computers = lib.mkIf cfg.manager {
      description = "Members' computers: the manager behind compute.${base}";
      wantedBy = [ "multi-user.target" ];
      after = [
        "network-online.target"
        "firewall.service"
      ];
      wants = [ "network-online.target" ];
      path = [ pkgs.systemd ];
      environment = {
        DD_COMPUTERS_DIR = dir;
        DD_COMPUTERS_BIND = sock;
        DD_COMPUTERS_PORT_BASE = toString cfg.portBase;
        DD_COMPUTERS_PORT_COUNT = toString cfg.portCount;
        DD_COMPUTERS_MEMORY_MIB = toString cfg.memoryMiB;
        DD_COMPUTERS_THREADS = toString cfg.threads;
        DD_COMPUTERS_RESERVE_MIB = toString cfg.reserveMiB;
        DD_COMPUTERS_ADDRESS = config.dd.box.net;
        # whose box: "my boxes only" runs here for them alone. A fleet box
        # (no owner of its own) is the fleet owner's
        DD_COMPUTERS_BOX_OWNER =
          if config.dd.box.owner != null then
            config.dd.box.owner
          else
            (lib.importJSON ../../../fleet/policy.json).owner;
        DD_COMPUTERS_ACCESS = "/run/dd-computers/access.json";
        # the storage ledger's count, where this box keeps one (storage/view.nix)
        DD_COMPUTERS_LEDGER = "/var/lib/dd-storage/ledger.json";
      };
      serviceConfig = {
        User = "dd-computers";
        Group = "dd-computers";
        RuntimeDirectory = "dd-computers";
        RuntimeDirectoryMode = "0750";
        # to read the ledger, where there is one
        SupplementaryGroups = lib.optional (config.users.groups ? dd-storage) "dd-storage";
        ExecStart = "${self.packages.${pkgs.stdenv.hostPlatform.system}.computers}/bin/dd-computers";
        Restart = "on-failure";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ dir ];
        ProtectHome = true;
        PrivateTmp = true;
      };
    };
    security.polkit.enable = lib.mkIf cfg.manager true;
    security.polkit.extraConfig = lib.mkIf cfg.manager ''
      polkit.addRule(function(action, subject) {
        if (action.id == "org.freedesktop.systemd1.manage-units" &&
            subject.user == "dd-computers" &&
            /^dd-computer@[0-9a-f]{16}\.service$/.test(action.lookup("unit")) &&
            ["start", "stop"].indexOf(action.lookup("verb")) >= 0) {
          return polkit.Result.YES;
        }
      });
    '';
    users.users.nginx = lib.mkIf (cfg.manager && config.services.nginx.enable) {
      extraGroups = [ "dd-computers" ];
    };
    services.nginx.virtualHosts."compute.${base}" = lib.mkIf cfg.manager {
      useACMEHost = base;
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://unix:${sock}:";
        extraConfig = ''
          auth_request /_dd/verify;
          auth_request_set $auth_user $upstream_http_x_auth_request_preferred_username;
          proxy_set_header X-DD-User $auth_user;
          auth_request_set $auth_role $upstream_http_x_dd_role;
          proxy_set_header X-DD-Role $auth_role;
          proxy_set_header Authorization "";
          client_max_body_size 128k;
          error_page 401 = @login;
          error_page 403 = @waiting;
        '';
      };
    };

    # Who may reach each computer: its owner's devices and the friends its
    # config names, asked of the gate every fifteen seconds, as for games
    # (games-access.sh, the same script on its own chain)
    systemd.services.computers-access = lib.mkIf cfg.manager {
      description = "Open each member's computer to its owner's and invited devices";
      after = [ "firewall.service" ];
      path = [
        pkgs.jq
        pkgs.curl
        pkgs.iptables
        pkgs.coreutils
        pkgs.diffutils
      ];
      serviceConfig.Type = "oneshot";
      script = ddScript ../games/games-access.sh {
        ACCESS = "/run/dd-computers/access.json";
        GATE = "http://127.0.0.1:${toString config.dd.verify.port}";
        STATE = "/run/computers-access";
        CHAIN = "dd-computers";
      };
    };
    systemd.timers.computers-access = lib.mkIf cfg.manager {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "30s";
        OnUnitActiveSec = "15s";
        AccuracySec = "1s";
      };
    };
    systemd.services.firewall.postStart = lib.mkIf cfg.manager (
      lib.mkAfter "systemctl start --no-block computers-access.service || true"
    );

    # what each member's computers keep here, into the facts for their allowance
    systemd.services.dd-computers-usage = {
      description = "Count what each member's computers keep";
      path = [
        pkgs.jq
        pkgs.coreutils
      ];
      restartIfChanged = false;
      serviceConfig.Type = "oneshot";
      script = ddScript ../guests/usage.sh {
        DIR = dir;
        KIND = "computers";
      };
    };
    systemd.timers.dd-computers-usage = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "5min";
        OnUnitActiveSec = "1h";
      };
    };

    # how big a computer this box can take, and what each here holds: into
    # the heartbeat, for placement (computer-facts.sh)
    systemd.services.dd-computer-facts = {
      description = "Say how big a computer this box can take";
      path = [
        pkgs.coreutils
        pkgs.gawk
      ];
      restartIfChanged = false;
      serviceConfig.Type = "oneshot";
      script = ddScript ./computer-facts.sh {
        DIR = dir;
        RESERVE_BYTES = cfg.reserveMiB * 1024 * 1024;
      };
    };
    systemd.timers.dd-computer-facts = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1min";
        OnUnitActiveSec = "1min";
      };
    };

    # SSH and forwards are open on the Commonty network only, to whom the
    # owner lets in; the chain is filled once there is a manager (step 2)
    networking.firewall.extraCommands = ''
      for ipt in iptables ip6tables; do
        $ipt -N dd-computers 2>/dev/null || true
        $ipt -D nixos-fw -i commonty0 -p tcp --dport ${range} -j dd-computers 2>/dev/null || true
        $ipt -I nixos-fw 1 -i commonty0 -p tcp --dport ${range} -j dd-computers
      done
    '';
  };
}
