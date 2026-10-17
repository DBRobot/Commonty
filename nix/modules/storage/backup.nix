{
  config,
  options,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.backup;
  box = config.networking.hostName;
  facts = "/var/lib/dd-facts";
  garage = config.services.garage.enable;
  # where the box can mail (modules/mail.nix)
  mail = options.dd ? mail;

  # After a good run: when it ended, and what the repository holds, so the
  # backups page can say how far back this box goes without anything but
  # this unit holding the repository password.
  #
  # Every line is written only once its value is known to be a number.
  # The textfile collector drops the whole file over one malformed line,
  # so a metric with an empty value would take the others with it.
  mark = pkgs.writeShellScript "backup-mark" ''
    mkdir -p ${facts}
    f=${facts}/backup.prom.tmp
    num() { # num <metric> <value>: a line, if the value is a number
      case "$2" in "" | *[!0-9]*) return 0 ;; esac
      printf 'dd_backup_%s{box="${box}"} %s\n' "$1" "$2" >> $f
    }
    num last_success_seconds "$(date +%s)"
    if snaps=$(${pkgs.restic}/bin/restic snapshots --json 2>/dev/null); then
      jq=${pkgs.jq}/bin/jq
      num snapshots "$(echo "$snaps" | $jq 'length' 2>/dev/null)"
      # restic stamps a local offset, which jq cannot read; date can, and
      # sorting the strings orders them but for the hour a clock change
      # moves, which nothing here cares about
      for which in 0 -1; do
        t=$(echo "$snaps" | $jq -r "map(.time) | sort | .[$which] // empty" 2>/dev/null)
        [ -n "$t" ] || continue
        [ "$which" = 0 ] && name=oldest || name=newest
        num "''${name}_seconds" "$(date -d "$t" +%s 2>/dev/null)"
      done
    fi
    mv $f ${facts}/backup.prom
  '';

  # Backups die silently by default. Where the box can mail, a failed run does.
  alert = pkgs.writeShellScript "dd-alert" ''
        set -eu
        unit="$1"
        ${config.dd.mail.sendmail} alerts <<EOF
    From: ${box} <alerts@${config.dd.domain}>
    Subject: ${box}: $unit FAILED

    $unit failed on ${box}.

      journalctl -u $unit -n 50
    EOF
  '';
in
{
  # A box backs itself up: restic, encrypted here with this box's own
  # password, into a bucket of its own in the garage cluster, with a key
  # that reaches no other bucket. The cluster keeps two copies, so the
  # backup survives the loss of any box, including this one. No box holds
  # another box's ssh key or password; what leaves a box is ciphertext.
  # Roles add the paths their state lives in. Garage's own blocks are not
  # here: the cluster replicates those itself, and its metadata snapshots
  # ride along so the cluster can be rebuilt from blocks plus snapshot.
  options.dd.backup = {
    paths = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "Directories on this box worth having tomorrow.";
    };
    exclude = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
    };
    endpoint = lib.mkOption {
      type = lib.types.str;
      default = config.dd.box.s3;
      description = "S3 endpoint: the storage pool at this box's address for it (dd.box.s3): its own garage, or a storage box's over the fleet's network";
    };
    envFile = lib.mkOption {
      type = lib.types.path;
      description = "AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY of this box's backup key.";
    };
    passwordFile = lib.mkOption {
      type = lib.types.path;
      description = "The restic repository password; nothing but this box and the release signer's sops key has it.";
    };
  };

  config = lib.mkMerge [
    {
      # a snapshot of the metadata db before each backup, shipped with it
      dd.backup.paths = lib.mkIf garage [ "/var/lib/garage/meta/snapshots" ];
      systemd.services.garage-meta-snapshot = lib.mkIf garage {
        description = "Snapshot garage's metadata db";
        after = [ "garage.service" ];
        requires = [ "garage.service" ];
        path = [
          config.services.garage.package
          pkgs.findutils
        ];
        serviceConfig = {
          Type = "oneshot";
          # As garage, not as root. The directory being pruned belongs to
          # garage, so the names `find` collects are names garage chooses,
          # and `rm -rf` looks each one up again a moment later - long
          # enough for its owner to have pointed it somewhere else. Root
          # doing that is how a compromised garage deletes /etc; garage
          # doing it can reach nothing it could not already reach.
          User = "garage";
          Group = "garage";
          EnvironmentFile = config.dd.garage.envFile;
        };
        script = ''
          garage meta snapshot
          # a week of them, as long as the blocks' own snapshots (garage.nix)
          find /var/lib/garage/meta/snapshots -mindepth 1 -maxdepth 1 -mtime +7 -exec rm -rf {} +
        '';
      };
    }
    (lib.mkIf (cfg.paths != [ ]) {
      # What this box covers, from boot, whether or not a run has ever
      # worked. Written by the backup only, it would say "nothing here is
      # backed up" about a box whose backups have never once succeeded,
      # which is the one case worth seeing.
      # the directory itself belongs to modules/observe/metrics.nix, which
      # owns it as node-exporter; a second `d` rule here would fight it
      # over the owner on every boot. L+ makes what parents it needs.
      systemd.tmpfiles.rules = [
        "L+ ${facts}/backup-paths.prom - - - - ${
          pkgs.writeText "backup-paths.prom" (
            "# HELP dd_backup_path A path this box backs up.\n# TYPE dd_backup_path gauge\n"
            + lib.concatMapStrings (p: "dd_backup_path{box=\"${box}\",path=\"${p}\"} 1\n") cfg.paths
          )
        }"
      ];

      services.restic.backups.dd = {
        repository = "s3:${cfg.endpoint}/backups-${box}";
        environmentFile = toString cfg.envFile;
        passwordFile = toString cfg.passwordFile;
        paths = cfg.paths;
        exclude = cfg.exclude;
        initialize = true;
        timerConfig = {
          OnCalendar = "03:00";
          RandomizedDelaySec = "1h";
          Persistent = true; # a box that was off at 03:00 backs up when it wakes
        };
        # forget --prune after every backup; at most about 5 weeks back
        # (nix/tests/retention.nix)
        pruneOpts = [
          "--keep-daily 7"
          "--keep-weekly 4"
        ];
        # read a slice of the data back every run: a repository that lists
        # fine and restores nothing is the common failure
        checkOpts = [ "--read-data-subset=2%" ];
        extraBackupArgs = [ "--one-file-system" ];
      };

      systemd.services.restic-backups-dd = {
        after = [
          "network-online.target"
        ]
        ++ lib.optionals garage [
          "garage-setup.service"
          "garage-meta-snapshot.service"
        ];
        wants = [ "network-online.target" ];
        requires = lib.optionals garage [ "garage-meta-snapshot.service" ];
        # samples, not events: the box dashboard shows the age of the last
        # good backup; ExecStartPost runs only when the backup succeeded
        serviceConfig.ExecStartPost = "+${mark}";
        onFailure = lib.optional mail "dd-alert@restic-backups-dd.service";
      };

      # a timer that stops firing is silent; this one is not. Fails when
      # the last good run is over a day old, which the box that can mail
      # mails, and every box shows as a failed unit on its dashboard.
      systemd.services.dd-backup-stale = {
        description = "Fail if the last good backup is over 26 hours old";
        startAt = "09:00";
        onFailure = lib.optional mail "dd-alert@dd-backup-stale.service";
        serviceConfig = {
          Type = "oneshot";
          # a backup it runs itself can take a while
          TimeoutStartSec = "6h";
        };
        script = ''
          stale() {
            last=$(${pkgs.gawk}/bin/awk '/^dd_backup_last_success_seconds/ {print $2}' ${facts}/backup.prom 2>/dev/null || echo 0)
            # that file is written by node-exporter, which is not root, and
            # this unit is. Bash expands command substitutions inside an
            # arithmetic context, so a value that is not a number is a way to
            # run one.
            case "$last" in "" | *[!0-9]*) last=0 ;; esac
            age=$(( $(date +%s) - last ))
            [ "$age" -gt $((26 * 3600)) ]
          }
          stale || exit 0
          # too old: the nightly timer may have lost its next run (node2's
          # did, 2026-10-12). Its timer again, and a backup now; mailed only
          # if that one did not make it either
          echo "last good backup was $((age / 3600)) hours ago: restarting the timer and backing up now"
          ${pkgs.systemd}/bin/systemctl restart restic-backups-dd.timer || true
          ${pkgs.systemd}/bin/systemctl start restic-backups-dd.service || true
          if stale; then
            echo "still: last good backup was $((age / 3600)) hours ago"; exit 1
          fi
          echo "backed up again"
        '';
      };

      systemd.services."dd-alert@" = lib.mkIf mail {
        description = "Mail out a failure of %i";
        serviceConfig = {
          Type = "oneshot";
          ExecStart = "${alert} %i";
        };
      };
    })
  ];
}
