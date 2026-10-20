{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.cache;
  # the storage pool at this box's address for it: its own garage, or a
  # storage box's over the tailnet (dd.box.s3)
  # what the post-build hook has queued for dd-cache-upload
  spool = "/var/lib/dd-cache-upload";
  url = "s3://nix-cache?endpoint=${lib.removePrefix "http://" config.dd.box.s3}&scheme=http&region=us-east-1";
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
    systemd.paths.dd-cache-upload = {
      wantedBy = [ "paths.target" ];
      pathConfig.DirectoryNotEmpty = "${spool}/queue";
    };
    systemd.services.dd-cache-upload = {
      description = "Upload what this box built to the nix cache, signed with its key";
      # a switch never cuts an upload short; the path unit starts the next
      restartIfChanged = false;
      environment = {
        HOME = "/root";
        AWS_SHARED_CREDENTIALS_FILE = cfg.credentialsFile;
      };
      path = [
        config.nix.package
        pkgs.coreutils
        pkgs.findutils
      ];
      serviceConfig = {
        Type = "oneshot";
        Nice = 19;
        IOSchedulingClass = "idle";
      };
      # never fails: the path unit would start it again at once, and the
      # cache is a copy, not the result
      script = ''
        set -u
        files=$(find ${spool}/queue -type f)
        [ -n "$files" ] || exit 0
        paths=()
        for p in $(cat $files | sort -u); do
          # gone already (a GC since), or not worth the space: a VM test's
          # 1.5 GB disk image and anything big are rebuilt in seconds
          [ -e "$p" ] || continue
          case $p in *-nixos-disk-image) continue ;; esac
          [ "$(du -s --block-size=1M "$p" | cut -f1)" -gt 256 ] && continue
          paths+=("$p")
        done
        rm -f $files
        [ ''${#paths[@]} -gt 0 ] || exit 0
        nix --extra-experimental-features nix-command \
          copy --to '${url}&secret-key=${cfg.signingKeyFile}' "''${paths[@]}" \
          || echo "cache upload failed for ''${#paths[@]} paths"
        exit 0
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
