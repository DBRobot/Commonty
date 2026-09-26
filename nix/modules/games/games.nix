{
  config,
  pkgs,
  lib,
  self,
  ...
}:
let
  cfg = config.dd.games;
  base = config.dd.domain;
  port = 4182; # only a test binds this; a box serves on the socket
  # the manager takes the caller's name from a header nginx sets from the
  # verifier. A port would let anything else on this box set it too, and
  # this box runs CI jobs and game guests
  sock = "/run/dd-games/dd-games.sock";
  dir = "/var/lib/dd-games";
  # steamcmd and what steam-run wraps are unfree. The guest gets a package
  # set of its own that allows exactly those, so the box's does not change
  # (and a test's, which is read-only, does not have to).
  guestPkgs = import pkgs.path {
    inherit (pkgs.stdenv.hostPlatform) system;
    overlays = [ (import ../../overlays/fhsenv.nix) ];
    config.allowUnfreePredicate =
      pkg:
      builtins.elem (lib.getName pkg) [
        "steamcmd"
        "steam-unwrapped"
        "steam-run"
        "steam-original"
      ];
  };
  guest = guestPkgs.nixos ./game-guest.nix;
  vm = guest.config.system.build.vm;

  # One instance, by id: read its record, give the guest its memory, cores
  # and ports, its own disk, and its directory. Everything that differs
  # between two servers is in the record; the machine is the same.
  run = pkgs.writeShellScript "dd-game-run" ''
    set -eu
    id="$1"
    d=${dir}/instances/"$id"
    rec="$d"/instance.json
    [ -s "$rec" ] || { echo "no such instance: $id" >&2; exit 1; }
    mem=$(${pkgs.jq}/bin/jq -r '.memory' "$rec")
    cores=$(${pkgs.jq}/bin/jq -r '.cores' "$rec")
    # every port both ways: games use udp and tcp on the same number, and
    # the guest listens on the number the host hands it
    fwd=$(${pkgs.jq}/bin/jq -r '[.ports[] | "hostfwd=udp::\(.port)-:\(.port)", "hostfwd=tcp::\(.port)-:\(.port)"] | join(",")' "$rec")
    rm -f "$d"/qmp "$d"/status
    export DD_INSTANCE_DIR="$d"
    export NIX_DISK_IMAGE="$d"/disk.qcow2
    export QEMU_NET_OPTS="$fwd"
    export QEMU_OPTS="-m $mem -smp $cores -qmp unix:$d/qmp,server,nowait ${lib.optionalString (!cfg.kvm) "-machine accel=tcg -cpu max"}"
    cd "$d"
    exec ${vm}/bin/run-game-vm
  '';
  # A clean stop: ask the guest to power down, which stops the game the way
  # its unit says; qemu exits when the guest has. systemd kills it past the
  # unit's stop timeout.
  stop = pkgs.writeShellScript "dd-game-stop" ''
    d=${dir}/instances/"$1"
    [ -S "$d"/qmp ] || exit 0
    printf '%s\n' '{"execute":"qmp_capabilities"}' '{"execute":"system_powerdown"}' \
      | ${pkgs.socat}/bin/socat - UNIX-CONNECT:"$d"/qmp >/dev/null 2>&1 || true
    # the guest gets a minute to shut the game down and power off; then
    # qemu is told to quit, which is the same as pulling the plug
    for i in $(seq 1 60); do kill -0 "$MAINPID" 2>/dev/null || exit 0; sleep 1; done
    printf '%s\n' '{"execute":"qmp_capabilities"}' '{"execute":"quit"}' \
      | ${pkgs.socat}/bin/socat - UNIX-CONNECT:"$d"/qmp >/dev/null 2>&1 || true
    for i in $(seq 1 15); do kill -0 "$MAINPID" 2>/dev/null || exit 0; sleep 1; done
  '';
in
{
  # Game servers a member starts for themselves. The release says what can
  # run and how: a catalogue of games, one guest machine (game-guest.nix), a
  # template unit that boots it. That a given server exists is not in the
  # repo: it is a record on this box, made at a member's request, and the
  # unit for it is an instance of the template. So a deploy never touches a
  # running game - the template is marked never to be restarted or stopped
  # by a switch, and a newer guest takes effect when that server next
  # restarts - and a reboot brings back every server whose record says it
  # should be up (the manager's job).
  options.dd.games = {
    enable = lib.mkEnableOption "game servers members start for themselves";
    catalogue = lib.mkOption {
      type = lib.types.path;
      default = ../../../data/games/catalogue.json;
      description = "every game that can run: the pelican eggs, resolved by games/resolve.py";
    };
    covers = lib.mkOption {
      type = lib.types.path;
      default = ../../../data/games/covers;
      description = "the games' covers, by steam app id, fetched once by games/resolve.py";
    };
    perMember = lib.mkOption {
      type = lib.types.int;
      default = 1;
      description = "servers one member may have up at once";
    };
    memoryMiB = lib.mkOption {
      type = lib.types.int;
      default = 16384;
      description = "memory every running guest on this box may have together";
    };
    portBase = lib.mkOption {
      type = lib.types.port;
      default = 27000;
      description = "first of the ports handed to instances";
    };
    portCount = lib.mkOption {
      type = lib.types.ints.positive;
      default = 200;
      description = "how many ports from portBase instances may be handed; the firewall opens exactly these on both tunnels";
    };
    kvm = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "guests use the box's virtualisation. Off only where the box is itself a guest whose nested kvm cannot boot one (the VM test on AMD).";
    };
    manager = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "run the manager and its page (a test of the template alone turns it off)";
    };
  };

  config = lib.mkIf cfg.enable {
    # Players reach a server on its own port, from either tunnel: the range
    # the manager hands out and nothing around it. Both protocols, because
    # games use udp and tcp on the same number.
    networking.firewall.interfaces =
      let
        range = [
          {
            from = cfg.portBase;
            to = cfg.portBase + cfg.portCount - 1;
          }
        ];
        open = {
          allowedTCPPortRanges = range;
          allowedUDPPortRanges = range;
        };
      in
      {
        tailscale0 = open;
        commonty0 = open;
      };
    dd.box.plaintext = [ "games (servers members start; their worlds)" ];

    # fixed ids: the guest's game user has the same, so a 9p share needs no
    # id mapping (modules/game-guest.nix)
    # nginx opens the manager's socket; naming users.users.nginx where nginx
    # does not run would leave a user with no group and no kind
    users.users.nginx = lib.mkIf config.services.nginx.enable {
      extraGroups = [ "dd-games" ];
    };
    users.users.dd-games = {
      uid = 951;
      isSystemUser = true;
      group = "dd-games";
      extraGroups = [ "kvm" ];
      home = dir;
    };
    users.groups.dd-games.gid = 951;
    systemd.tmpfiles.rules = [
      "d ${dir} 0750 dd-games dd-games -"
      "d ${dir}/instances 0750 dd-games dd-games -"
    ];

    # The manager: makes and removes the records, starts and stops the
    # instances, and on its own start brings back every server that should
    # be up. It runs as the games user and may do exactly this to systemd:
    # start and stop instances of the template.
    systemd.services.dd-games = lib.mkIf cfg.manager {
      description = "Game servers members start for themselves";
      wantedBy = [ "multi-user.target" ];
      # where a box keeps this on a dataset (node1: tank/games), wait for it;
      # the dataset arrives owned by root, so the directories are made here
      after = [
        "network-online.target"
        "zfs-datasets.service"
      ];
      wants = [
        "network-online.target"
        "zfs-datasets.service"
      ];
      path = [ pkgs.systemd ];
      environment = {
        DD_GAMES_DIR = dir;
        DD_GAMES_CATALOGUE = cfg.catalogue;
        DD_GAMES_COVERS = cfg.covers;
        DD_GAMES_BIND = sock;
        DD_GAMES_PORT_BASE = toString cfg.portBase;
        DD_GAMES_PORT_COUNT = toString cfg.portCount;
        DD_GAMES_PER_MEMBER = toString cfg.perMember;
        DD_GAMES_MEMORY_MIB = toString cfg.memoryMiB;
        DD_GAMES_ADDRESS = config.dd.box.tailnet;
        DD_GAMES_HOME = "https://home.${base}/";
      };
      serviceConfig = {
        User = "dd-games";
        Group = "dd-games";
        RuntimeDirectory = "dd-games";
        RuntimeDirectoryMode = "0750";
        ExecStartPre = "+${pkgs.coreutils}/bin/install -d -o dd-games -g dd-games -m 0750 ${dir} ${dir}/instances";
        ExecStart = "${self.packages.${pkgs.stdenv.hostPlatform.system}.games}/bin/dd-games";
        Restart = "on-failure";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ dir ];
        ProtectHome = true;
        PrivateTmp = true;
      };
    };
    security.polkit.enable = true;
    security.polkit.extraConfig = ''
      polkit.addRule(function(action, subject) {
        if (action.id == "org.freedesktop.systemd1.manage-units" &&
            subject.user == "dd-games" &&
            /^dd-game@[a-z0-9]+\.service$/.test(action.lookup("unit")) &&
            ["start", "stop"].indexOf(action.lookup("verb")) >= 0) {
          return polkit.Result.YES;
        }
      });
    '';

    # the page, behind the gate like every other; the manager is told who
    # is asking by the name the verifier answered with
    services.nginx.virtualHosts."games.${base}" = lib.mkIf cfg.manager {
      useACMEHost = base;
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://unix:${sock}:";
        extraConfig = ''
          auth_request /_dd/verify;
          auth_request_set $auth_user $upstream_http_x_auth_request_preferred_username;
          proxy_set_header X-DD-User $auth_user;
          # likewise: the manager is told who, not handed the proof
          proxy_set_header Authorization "";
          proxy_set_header Cookie $dd_cookie_stripped;
          error_page 401 = @login;
          error_page 403 = @waiting;
        '';
      };
    };

    systemd.services."dd-game@" = {
      description = "Game server %i";
      # a deploy is not a reason for a game to go down
      restartIfChanged = false;
      stopIfChanged = false;
      unitConfig.X-StopOnRemoval = false;
      serviceConfig = {
        User = "dd-games";
        Group = "dd-games";
        ExecStart = "${run} %i";
        ExecStop = "${stop} %i";
        TimeoutStopSec = 90;
        Restart = "on-failure";
        RestartSec = 20;
        # the guest is the boundary; this keeps qemu itself in its lane
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ "${dir}/instances" ];
        ProtectHome = true;
        PrivateTmp = true;
        DeviceAllow = [ "/dev/kvm rw" ];
      };
    };
  };
}
