# The machine a game server runs in: a small NixOS guest under qemu, the same
# for every game and every instance. What makes it one game and not another
# is a file the host hands it, /instance/instance.json: the steam app to
# install, the program to run, its arguments, where its saves live. The
# program is a big closed binary, often modded; it gets a kernel of its own,
# a disk of its own, and the network qemu gives it, and nothing of the box.
{
  config,
  pkgs,
  lib,
  modulesPath,
  ...
}:
{
  imports = [ "${modulesPath}/virtualisation/qemu-vm.nix" ];

  options.dd.gameGuest.wine = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = "wine in the guest, for the windows servers among the eggs";
  };

  config = {
    system.stateVersion = "26.05";

    virtualisation = {
      graphics = false;
      # the host overrides both for the instance (QEMU_OPTS: the last -m wins)
      memorySize = 4096;
      cores = 2;
      # sparse: the game's files and steam's live here, per instance
      diskSize = 65536;
      # the instance's directory on the host: its description, and its saves,
      # which so stay ordinary files the box can back up
      sharedDirectories.instance = {
        source = ''"$DD_INSTANCE_DIR"'';
        target = "/instance";
        # files keep the host's ownership: qemu runs as the box's games user,
        # which owns the instance, and the guest's game user has the same id
        securityModel = "none";
      };
    };

    networking.hostName = "game";
    networking.firewall.enable = false; # qemu forwards exactly the instance's ports
    documentation.enable = false;
    services.getty.autologinUser = lib.mkForce null;

    users.users.game = {
      isSystemUser = true;
      uid = 951;
      group = "game";
      home = "/var/lib/game";
      createHome = true;
    };
    users.groups.game.gid = 951;
    # the eggs install into /mnt/server and run from /home/container: both
    # are the instance's server directory on the share
    systemd.tmpfiles.rules = [
      "d /mnt 0755 root root -"
      "L+ /mnt/server - - - - /instance/server"
      "d /home 0755 root root -"
      "L+ /home/container - - - - /instance/server"
    ];

    # what the eggs' install scripts and startup lines reach for, and the
    # runtimes some servers run on: dotnet (cryofall, resonite,
    # v-rising-bepinex), mono (neosvr, subnautica), java (rising-world-java-
    # legacy; necesse and project-zomboid bring their own)
    environment.systemPackages =
      with pkgs;
      [
        dotnetCorePackages.runtime_8_0
        dotnetCorePackages.runtime_9_0
        mono
        jre_headless
        python3
        curl
        wget
        unzip
        gnutar
        gzip
        xz
        zstd
        jq
        git
        git-lfs
        file
        which
        procps
        iproute2 # ss, for the runner's port watch
      ]
      ++ lib.optionals config.dd.gameGuest.wine (
        with pkgs;
        [
          xorg.xorgserver # Xvfb, for the windows servers
          xvfb-run
          wineWowPackages.stable
          winetricks
          # the eggs say 'proton run x.exe'; wine is what proton is underneath
          (writeShellScriptBin "proton" ''
            [ "$1" = run ] && shift
            exec wine "$@"
          '')
        ]
      );

    systemd.services.game = {
      description = "The game this instance is";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = [
        pkgs.steamcmd
        pkgs.steam-run
        pkgs.bash
        "/run/current-system/sw"
      ];
      serviceConfig = {
        User = "game";
        WorkingDirectory = "/var/lib/game";
        Restart = "on-failure";
        RestartSec = 15;
        # the game's own output, readable on the box beside the record
        StandardOutput = "append:/instance/game.log";
        StandardError = "inherit";
        # the egg's stop command goes down the game's stdin; give it a minute
        KillSignal = "SIGTERM";
        TimeoutStopSec = 75;
      };
      environment = {
        HOME = "/var/lib/game";
        # steam-run wraps the game in steam's runtime, which is what the eggs'
        # debian containers give it
        STEAM_RUNTIME = "1";
      };
      script = ''
        exec steam-run ${pkgs.python3}/bin/python3 ${./game-run.py}
      '';
    };
  };
}
