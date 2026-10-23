{
  config,
  pkgs,
  lib,
  self,
  ...
}:
let
  cfg = config.dd.agent;
  # the home controller (nix/controller) runs the agent with no dd.box: a
  # device of a household, never a box that joined on its own key
  joined = config.dd.box.joined or false;
  # the names a soaking box probes while it holds their service: each name
  # of a service with one writer (docs/dev/rollout.md)
  needs = import ../../lib/needs.nix {
    inherit lib;
    on = config.dd.failover or false;
  };
  siteProbes = lib.concatLists (
    lib.mapAttrsToList (
      svc: n:
      map (h: {
        flag = svc;
        url = "https://${h}.${config.dd.domain}/";
        resolve = "${h}.${config.dd.domain}:443:127.0.0.1";
      }) n.hosts
    ) (lib.filterAttrs (_: n: n.lease) needs)
  );
  agent = self.packages.${pkgs.stdenv.hostPlatform.system}.agent;
in
{
  imports = [
    ./work-memory.nix
    # what the probe leaves out (/etc/dd/remote-jobs)
    ./remote-jobs.nix
  ];

  # The box moves itself to the signed release and to nothing else. A timer
  # runs the agent; it fetches the release file, checks the signature
  # against the release key it was built with, refuses anything older than
  # what it last applied, fetches its own closure, checks the hash,
  # switches, probes, and rolls back if the box is not a box afterwards.
  # Nothing pushes to a box.
  options.dd.agent = {
    enable = lib.mkEnableOption "the release agent";
    url = lib.mkOption {
      type = lib.types.str;
      description = "Where the signed release file is.";
    };
    mirrors = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "More places that keep the signed release, read beside url (docs/dev/releases.md, \"Where a box finds the release\"). The newest file that verifies wins, wherever it came from; one holding an older release is handed the newer with a PUT.";
    };
    publicKey = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "The release key's public half, base64: fleet/release.pub. Written to publicKeyFile.";
    };
    publicKeyFile = lib.mkOption {
      type = lib.types.str;
      default = "/etc/dd/release.pub";
      description = "Where the agent reads the release key from; the vm test drops a key made at run time here.";
    };
    cache = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Binary cache the closures come from; null means they are already in the store.";
    };
    cacheEnvFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY for an s3 cache.";
    };
    interval = lib.mkOption {
      type = lib.types.str;
      default = "5m";
    };
    reach = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "host:port of the other boxes. A release that costs this box its reach to all of them is rolled back, however healthy the box looks from inside. Empty: a fleet of one, nothing to compare.";
    };
    attesters = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      description = "The fleet's boxes and their ssh host keys (fleet/boxes.json): a release moves this box only to a closure two of them vouched for. Fewer than two: a fleet that cannot vouch, and nothing is asked.";
    };
    verifyPort = lib.mkOption {
      type = lib.types.port;
      default = 4181;
      description = "What must answer on 127.0.0.1 after a switch: the gate on a box, the door on a home controller.";
    };
    ssh = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Whether sshd must be up after a switch. A home controller runs none.";
    };
    rollout = lib.mkOption {
      type = lib.types.bool;
      # off until validators keep one version through it: release 356 left
      # node1 and node2 on different control planes, their states split and
      # consensus stopped (2026-10-22). Every box takes a release at once
      default = false;
      description = "Follow the staged rollout the control plane agrees (docs/dev/rollout.md): hold a release until this box's wave, drain before the switch, soak after it. Off: a release is taken at once.";
    };
    drainSeconds = lib.mkOption {
      type = lib.types.int;
      default = 600;
      description = "How long a drain may take before the box switches anyway, what is left restarting in place.";
    };
    probeSeconds = lib.mkOption {
      type = lib.types.int;
      default = 600;
      description = "How long the box has to come up healthy after a switch before the previous system comes back.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.etc."dd/release.pub" = lib.mkIf (cfg.publicKey != null) {
      text = cfg.publicKey + "\n";
    };
    # whose word on a closure counts, as this system was built: a release
    # cannot add a box to it for itself
    environment.etc."dd/attesters.json".text = builtins.toJSON cfg.attesters;

    # this box's word on a release before it is signed: `dd release
    # publish` runs it over ssh with sudo (servers/system/release/src/bin/dd-attest.rs)
    environment.systemPackages = [
      (pkgs.writeShellScriptBin "dd-attest" ''
        export PATH=${
          lib.makeBinPath [
            config.nix.package
            pkgs.git
            pkgs.openssh
            pkgs.coreutils
          ]
        }
        exec ${agent}/bin/dd-attest "$@"
      '')
    ];

    systemd.services.dd-agent = {
      description = "Move this box to the signed release";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      # the switch it performs must never restart the unit performing it
      restartIfChanged = false;
      stopIfChanged = false;
      path = [
        config.nix.package
        pkgs.systemd
        pkgs.coreutils
        pkgs.openssh # ssh-keygen checks the boxes' signatures
        pkgs.curl # a soak's probes of the names this box serves
      ]
      # a box that joined fetches the signed commit to build itself from
      ++ lib.optional joined pkgs.git;
      environment = {
        DD_AGENT_URL = cfg.url;
        DD_AGENT_MIRRORS = lib.concatStringsSep " " cfg.mirrors;
        DD_AGENT_ATTESTERS = "/etc/dd/attesters.json";
        DD_AGENT_KEY_FILE = cfg.publicKeyFile;
        DD_AGENT_PROBE_SECS = toString cfg.probeSeconds;
        DD_AGENT_VERIFY_PORT = toString cfg.verifyPort;
        DD_AGENT_SSH = lib.boolToString cfg.ssh;
        DD_AGENT_REACH = lib.concatStringsSep " " cfg.reach;
      }
      // lib.optionalAttrs (cfg.cache != null) { DD_AGENT_CACHE = cfg.cache; }
      // lib.optionalAttrs cfg.rollout {
        DD_AGENT_CONTROL = "http://127.0.0.1:4195";
        DD_AGENT_DRAIN_SECS = toString cfg.drainSeconds;
        DD_AGENT_SITE_PROBES = "/etc/dd/site-probes.json";
        DD_AGENT_PROMETHEUS = lib.optionalString config.services.prometheus.enable "http://127.0.0.1:${toString config.services.prometheus.port}";
      }
      # a box that joined on its own key builds its own system, from the
      # fleet's public repository at the commit the release key signed
      // lib.optionalAttrs joined {
        DD_AGENT_JOINED_SOURCE = "git+https://code.${config.dd.domain}/${config.dd.repo}";
        DD_AGENT_JOINED_DIR = "/var/lib/dd/box";
      };
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${agent}/bin/dd-agent";
        StateDirectory = "dd-agent";
        # nix runs as root here and talks to the store itself, so the
        # credentials for the s3 cache are the ones in this environment
        EnvironmentFile = lib.optional (cfg.cacheEnvFile != null) cfg.cacheEnvFile;
      };
    };
    # Build orders (docs/dev/app-release.md): the release key names which boxes
    # build what; a box named builds it, rebuilds what it did not build
    # itself, and puts its signed word in the agreed log through
    # dd-control. Beside the release file, fetched the same way.
    systemd.services.dd-build = {
      description = "Build what a signed build order asks of this box";
      # a build in progress is not stopped or restarted by a switch, and is
      # not part of whether a switch came up
      restartIfChanged = false;
      stopIfChanged = false;
      after = [
        "network-online.target"
        "dd-control.service"
      ];
      wants = [ "network-online.target" ];
      path = [
        config.nix.package
        pkgs.git
        pkgs.openssh
        pkgs.coreutils
      ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = lib.concatStringsSep " " [
          "${agent}/bin/dd-attest"
          # a release's order, and the nightly check's
          "--order ${lib.removeSuffix "current.json" cfg.url}build-order.json"
          "--order ${lib.removeSuffix "current.json" cfg.url}build-check.json"
          "--key-file ${cfg.publicKeyFile}"
        ];
        StateDirectory = "dd-attest";
        # an hour's build is normal; a box that has gone quiet in one is not
        TimeoutStartSec = "6h";
      };
    };
    # it builds as root, outside the daemon and its limit: under the work
    # slice's with CI. A kill ends this run; the timer starts the next
    dd.work.units.dd-build = { };
    systemd.timers.dd-build = {
      wantedBy = [ "timers.target" ];
      # never inside a switch: its first run is three minutes after the
      # timer starts, whether that is boot or the switch that brought it
      timerConfig = {
        OnActiveSec = "3m";
        OnUnitInactiveSec = "2m";
      };
    };

    environment.etc."dd/site-probes.json" = lib.mkIf cfg.rollout {
      text = builtins.toJSON siteProbes;
    };
    # a soaking box looks at itself every minute (docs/dev/rollout.md); one
    # that is not, every interval as before
    systemd.services.dd-agent-soak = lib.mkIf cfg.rollout {
      description = "Run the release agent again while this box soaks a release";
      restartIfChanged = false;
      unitConfig.ConditionPathExists = "/var/lib/dd-agent/soaking";
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${pkgs.systemd}/bin/systemctl start --no-block dd-agent.service";
      };
    };
    systemd.timers.dd-agent-soak = lib.mkIf cfg.rollout {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1m";
        OnUnitActiveSec = "1m";
      };
    };

    systemd.timers.dd-agent = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "2m";
        OnUnitActiveSec = cfg.interval;
        RandomizedDelaySec = "30s";
      };
    };
  };
}
