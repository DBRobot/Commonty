{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.cache;
  inherit (cfg) url;
  # what the post-build hook has queued for dd-cache-upload
  spool = "/var/lib/dd-cache-upload";
  # the storage pool at this box's address for it: its own garage, or a
  # storage box's over the tailnet (dd.box.s3)
  s3 = "s3://nix-cache?endpoint=${lib.removePrefix "http://" config.dd.box.s3}&scheme=http&region=us-east-1";
  # The laptop's, and this box's own. A box's nix installs whatever it
  # substitutes, so a key in this list is a machine allowed to decide what
  # the box runs - and node2 is meant to sit in someone else's house. So
  # never another box's: no box takes a build on another's word. Its own
  # key it may believe, since it signed only what it built (a box rebuilds
  # what a GC took from its store from its own uploads, not from scratch).
  # The agent checks a release's hashes and the whole closure under them
  # either way.
  keys = import ../../lib/cache-keys.nix {
    inherit lib;
    host = config.networking.hostName;
    listed = lib.fileContents ../../../fleet/cache-keys.pub;
  };
in
{
  # The nix cache is the nix-cache bucket in the cluster. A box with this
  # module substitutes from it and, through a post-build hook, uploads
  # whatever it builds, signed with its own key. The laptop signs what it
  # publishes with its key. A box's nix accepts the laptop's key and its
  # own (fleet/cache-keys.pub); the agent does not rely on them, it checks nar hashes from
  # the signed release and every path under it, so a poisoned upload can at
  # most mislead CI on the box that reads it, never a release.
  options.dd.cache = {
    signingKeyFile = lib.mkOption {
      type = lib.types.str;
      description = "This box's nix signing key, root-only.";
    };
    credentialsFile = lib.mkOption {
      type = lib.types.str;
      default = "/root/.aws/credentials";
      description = "AWS credentials file with this box's cache key, for nix as root.";
    };
    url = lib.mkOption {
      type = lib.types.str;
      default = s3;
      description = "The cache's nix store url; a test points it at a file:// cache.";
    };
    uploadTimeout = lib.mkOption {
      type = lib.types.ints.positive;
      default = 120;
      description = "Seconds one upload may take, plus one per MiB of the closure it may send; one cut off is tried again.";
    };
    retryDelay = lib.mkOption {
      type = lib.types.ints.positive;
      default = 30;
      internal = true;
      description = "Seconds before a failed upload is tried again, doubling each time up to half an hour.";
    };
  };

  config = {
    nix.settings = {
      substituters = [ url ];
      trusted-public-keys = keys;
      # Nix holds a build until its hook returns, and the upload is xz at
      # about 4 MB/s (a 174M test binary: 37-46s on node1, 2026-10-20). So the
      # hook only queues what was built; dd-cache-upload sends it after.
      post-build-hook = pkgs.writeShellScript "queue-for-cache" ''
        # a hook that fails fails the build; the cache is never worth that
        export PATH=${pkgs.coreutils}/bin
        { mkdir -p ${spool}/new ${spool}/queue && t=$(mktemp -p ${spool}/new) \
          && printf '%s\n' $OUT_PATHS > "$t" && mv "$t" ${spool}/queue/; } \
          || echo "not queued for the cache: $OUT_PATHS" >&2
      '';
    };
    systemd.tmpfiles.rules = [
      "d ${spool} 0700 root root -"
      "d ${spool}/new 0700 root root -"
      "d ${spool}/queue 0700 root root -"
    ];
    # A new name for the path unit: the old one went failed with the
    # service's start limit (2026-10-22), and a switch starts a new unit
    # where it might leave a failed one as it is.
    systemd.paths.dd-cache-queue = {
      wantedBy = [ "paths.target" ];
      pathConfig = {
        DirectoryNotEmpty = "${spool}/queue";
        Unit = "dd-cache-upload.service";
      };
    };
    systemd.services.dd-cache-upload = {
      description = "Upload what this box built to the nix cache, signed with its key";
      # a switch never cuts an upload short; the path unit starts the next
      restartIfChanged = false;
      # Every build queues a file, and the path unit starts this whenever
      # the queue is not empty and it is not running: past five starts in
      # ten seconds systemd refused it for good, and nothing more was sent.
      # It runs until the queue is empty and never fails (cache-upload.sh).
      startLimitIntervalSec = 0;
      environment = {
        HOME = "/root";
        AWS_SHARED_CREDENTIALS_FILE = cfg.credentialsFile;
        NIX_CONFIG = "extra-experimental-features = nix-command";
        SPOOL = spool;
        CACHE = "${url}${if lib.hasInfix "?" url then "&" else "?"}secret-key=${cfg.signingKeyFile}";
        TIMEOUT = toString cfg.uploadTimeout;
        RETRY_DELAY = toString cfg.retryDelay;
        MAX_TRIES = "24";
        # not worth the space on its own: a VM test's disk image and
        # anything this big are rebuilt in seconds
        MAX_MB = "256";
      };
      path = [
        config.nix.package
        pkgs.coreutils
        pkgs.findutils
      ];
      serviceConfig = {
        Type = "simple";
        Nice = 19;
        IOSchedulingClass = "idle";
      };
      # with no start limit, a script that broke would be started again at
      # once, for ever: it waits a minute first
      script = ''
        ${pkgs.bash}/bin/bash ${./cache-upload.sh} && exit 0
        echo "cache upload stopped (status $?); again in a minute" >&2
        sleep 60
      '';
    };
    # the daemon substitutes as root; its credentials live at the usual place
    systemd.services.nix-daemon.environment.AWS_SHARED_CREDENTIALS_FILE = cfg.credentialsFile;
    systemd.services.nix-daemon.environment.HOME = "/root";

    # The cache is held to what is still wanted, or it grows for good (95G
    # in its first two weeks): the closures of the last ten releases, and
    # anything a week old or newer. Run where the forge is, which has the
    # releases branch to say what each release was.
    systemd.services.dd-cache-gc = lib.mkIf config.services.forgejo.enable {
      description = "Let go of what no recent release wants from the nix cache";
      path = [
        config.nix.package
        pkgs.awscli2
        pkgs.jq
        pkgs.gitMinimal
        pkgs.coreutils
        pkgs.gawk
      ];
      environment = {
        HOME = "/root";
        AWS_SHARED_CREDENTIALS_FILE = cfg.credentialsFile;
        AWS_DEFAULT_REGION = "us-east-1";
        REPO = "${config.services.forgejo.repositoryRoot}/${config.dd.repo}.git";
        KEEP = "10";
        AGE = "7";
        CACHE = url;
        BUCKET = "nix-cache";
        ENDPOINT = config.dd.box.s3;
      };
      # with failover every box able to hold the forge has it, but only the
      # holder's releases branch is current: a box that held it once keeps a
      # stale copy, whose older releases would let go of what is wanted now
      unitConfig = lib.mkIf (config.dd.failover or false) {
        ConditionPathExists = "/run/dd-placed/forgejo";
      };
      serviceConfig = {
        Type = "oneshot";
        PrivateTmp = true;
      };
      script = ''
        TMP=$(mktemp -d)
        export TMP
        exec ${pkgs.bash}/bin/bash ${./cache-gc.sh}
      '';
    };
    systemd.timers.dd-cache-gc = lib.mkIf config.services.forgejo.enable {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = "daily";
        RandomizedDelaySec = "2h";
        Persistent = true;
      };
    };
  };
}
