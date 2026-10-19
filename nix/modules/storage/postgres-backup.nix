# The databases on this box, dumped hourly onto this box's own disk, by the
# box that runs them: no other role reaches into another service's
# database for it (storage/postgres-isolation.nix). Wherever postgres runs
# (roles import this through core), every database it holds is dumped.
{
  config,
  options,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.dumps;
  dir = "${cfg.dir}/postgres";
  dbs = config.services.postgresql.ensureDatabases;
  # ente's dump is restored weekly to prove it: its key hierarchy is the one
  # thing without which the photos cannot be decrypted by anyone
  ente = builtins.elem "ente" dbs;
  # where the box can mail (modules/mail.nix)
  mail = options.dd ? mail;
  box = config.networking.hostName;
  # services.postgresqlBackup writes <db>.sql.zstd and keeps one .prev. History
  # beyond that comes from zfs snapshots on the dataset, which is a mechanism
  # already trusted with the photos.
  dump = "${dir}/ente.sql.zstd";

  # The upstream module does the dump. These add the things it does not do, and
  # they are deliberately additive: if verification breaks, the backup still runs.
  verify = pkgs.writeShellScript "pg-backup-verify" ''
    set -euo pipefail
    psql=${config.services.postgresql.package}/bin/psql
    [ -s ${dump} ] || { echo "no dump at ${dump}"; exit 1; }

    # restoring into a scratch database is the only thing that proves a dump is
    # restorable rather than merely present
    # Restored by a role that can do nothing, connected as it - not switched
    # into from a superuser session, which a function body could switch back
    # out of. What ente's database contains is up to whoever can write to
    # it, and the dump carries it: an index built on a function runs that
    # function during the restore, with the restoring role's rights.
    $psql -q -c 'drop database if exists ente_restore_test' postgres
    $psql -q -c 'create database ente_restore_test owner ente_restore' postgres
    # belt and braces: never feed a dump containing \connect into psql
    if ${pkgs.zstd}/bin/zstd -dc ${dump} | ${pkgs.gnugrep}/bin/grep -qE '^\\connect'; then
      echo "dump contains \\connect - refusing to restore, it would target the live db"
      exit 1
    fi
    ${pkgs.zstd}/bin/zstd -dc ${dump} | $psql -q -v ON_ERROR_STOP=1 -U ente_restore -d ente_restore_test >/dev/null

    # the key hierarchy is the thing worth asserting on: without these rows the
    # blobs on vault cannot be decrypted by anyone
    keys=$($psql -tA -U ente_restore -d ente_restore_test -c 'select count(*) from key_attributes')
    files=$($psql -tA -U ente_restore -d ente_restore_test -c 'select count(*) from collection_files')
    $psql -q -c 'drop database ente_restore_test' postgres

    [ "$keys" -ge 1 ] || { echo "restored dump has $keys key_attributes rows"; exit 1; }
    echo "ok: key_attributes=$keys collection_files=$files"
  '';

  # Backups usually die silently rather than loudly.
  alert = pkgs.writeShellScript "pg-backup-alert" ''
        set -eu
        unit="$1"
        ${config.dd.mail.sendmail} alerts <<EOF
    From: ${box} <alerts@${config.dd.domain}>
    Subject: ${box}: $unit FAILED

    $unit failed on ${box}.

      journalctl -u $unit -n 50

    These dumps are the only copy of what is in this box's databases, ente's
    key hierarchy among them: without it the photos cannot be decrypted by
    anyone, recovery key included.
    EOF
  '';
in
{
  options.dd.dumps.dir = lib.mkOption {
    type = lib.types.str;
    default = "/vault/backups";
    description = "where this box's database dumps go: the mountpoint of <bulk pool>/enc/backups, and in its restic backup";
  };

  config = lib.mkIf config.services.postgresql.enable (
    lib.mkMerge [
      # its own dataset in the bulk pool, snapshotted, under the encrypted root
      (lib.optionalAttrs (options.dd ? zfs && options.dd ? box) {
        dd.zfs.datasets."${config.dd.box.bulkPool}/enc/backups" = {
          mountpoint = cfg.dir;
          recordsize = "128K"; # small compressible dumps, not media
          compression = "zstd";
          "com.sun:auto-snapshot" = "true";
        };
      })
      (lib.optionalAttrs (options.dd ? backup) { dd.backup.paths = [ cfg.dir ]; })
      {
        # The dump itself is upstream's, not ours. It writes atomically via an
        # .in-progress file and runs as the postgres user.
        services.postgresqlBackup = {
          enable = true;
          databases = dbs;
          location = dir;
          # NOT the default "-C". That emits CREATE DATABASE + \connect <db>, so
          # restoring the dump into a scratch database would follow the \connect
          # and replay it into the LIVE one. Without -C the dump is portable and
          # lands wherever psql is pointed.
          pgdumpOptions = "--no-owner --no-privileges";
          compression = "zstd";
          compressionLevel = 6;
          startAt = "hourly";
        };

        systemd.services = lib.genAttrs (map (d: "postgresqlBackup-${d}") dbs) (_: {
          unitConfig.RequiresMountsFor = cfg.dir;
          # a dump of what runs, never a reason to run it: upstream's
          # Requires=postgresql.target made the hourly timer start postgres,
          # and a start queued while postgres is stopped for its lease to move
          # cancels that stop and brings it back on the old holder
          requires = lib.mkForce [ ];
          after = [ "postgresql.service" ];
          serviceConfig.ExecCondition = "${pkgs.systemd}/bin/systemctl is-active --quiet postgresql.service";
          serviceConfig.ExecStartPre = [
            # "+" runs as root despite User=postgres, so it can create the
            # directory inside the root-owned dataset mountpoint. Doing it here
            # rather than via tmpfiles avoids racing zfs-datasets on first boot.
            "+${pkgs.coreutils}/bin/install -d -o postgres -g postgres -m 0700 ${dir}"
          ];
        });
      }
      # a box that can send mail says when a dump fails
      (lib.mkIf mail {
        systemd.services =
          lib.genAttrs (map (d: "postgresqlBackup-${d}") dbs) (unit: {
            onFailure = [ "postgres-backup-alert@${unit}.service" ];
          })
          // {
            "postgres-backup-alert@" = {
              description = "Mail out a backup failure for %i";
              serviceConfig = {
                Type = "oneshot";
                ExecStart = "${alert} %i";
              };
            };
          };
      })
      (lib.mkIf ente {
        # the role the restore check runs as: it may log in and own the scratch
        # database, and nothing else. The check itself runs as postgres, so the
        # postgres OS user is allowed to be it, for that database alone.
        services.postgresql = {
          ensureUsers = [ { name = "ente_restore"; } ];
          identMap = ''
            restore postgres ente_restore
          '';
          authentication = ''
            local ente_restore_test ente_restore peer map=restore
          '';
        };
        systemd.services.postgres-backup-verify = {
          description = "Restore the newest dump into a scratch database and check it";
          after = [ "postgresql.service" ];
          onFailure = lib.optional mail "postgres-backup-alert@postgres-backup-verify.service";
          # not on the hour: the hourly dump is being rewritten then, and the
          # check found no file (2026-09-14)
          startAt = "Mon *-*-* 00:30:00";
          serviceConfig = {
            Type = "oneshot";
            User = "postgres";
            # as the dumps: only where postgres runs, never starting it
            ExecCondition = "${pkgs.systemd}/bin/systemctl is-active --quiet postgresql.service";
            ExecStart = verify;
          };
        };
      })
    ]
  );
}
