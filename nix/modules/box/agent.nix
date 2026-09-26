{
  config,
  pkgs,
  lib,
  self,
  ...
}:
let
  cfg = config.dd.agent;
  agent = self.packages.${pkgs.stdenv.hostPlatform.system}.agent;
in
{
  # The box moves itself to the signed release and to nothing else. A timer
  # runs the agent; it fetches the release file, checks the signature
  # against the release key it was built with, refuses anything older than
  # what it last applied, fetches its own closure, checks the hash,
  # switches, probes, and rolls back if the box is not a box afterwards.
  # Nothing pushes to a box. bin/deploy stays as the hand on the wheel.
  options.dd.agent = {
    enable = lib.mkEnableOption "the release agent";
    url = lib.mkOption {
      type = lib.types.str;
      description = "Where the signed release file is.";
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
    probeSeconds = lib.mkOption {
      type = lib.types.int;
      default = 600;
      description = "How long the box has to come up healthy after a switch before the previous system comes back.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.etc."dd/release.pub" = lib.mkIf (cfg.publicKey != null) { text = cfg.publicKey + "\n"; };

    # this box's word on a release before it is signed: `dd release
    # publish` runs it over ssh with sudo (box/release/src/bin/dd-attest.rs)
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
      ];
      environment = {
        DD_AGENT_URL = cfg.url;
        DD_AGENT_KEY_FILE = cfg.publicKeyFile;
        DD_AGENT_PROBE_SECS = toString cfg.probeSeconds;
        DD_AGENT_VERIFY_PORT = "4181";
        DD_AGENT_REACH = lib.concatStringsSep " " cfg.reach;
      }
      // lib.optionalAttrs (cfg.cache != null) { DD_AGENT_CACHE = cfg.cache; };
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${agent}/bin/dd-agent";
        StateDirectory = "dd-agent";
        # nix runs as root here and talks to the store itself, so the
        # credentials for the s3 cache are the ones in this environment
        EnvironmentFile = lib.optional (cfg.cacheEnvFile != null) cfg.cacheEnvFile;
      };
    };
    systemd.timers.dd-agent = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "2m";
        OnUnitActiveSec = cfg.interval;
        RandomizedDelaySec = "30s";
      };
    };
  };
}
