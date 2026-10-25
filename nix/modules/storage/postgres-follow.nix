# Postgres follows its lease (needs lease; box/placed.nix): the box holding
# svc/postgresql writes, and archives every WAL segment, encrypted here,
# into the store under its own epoch (s3://<bucket>/<group>/e<epoch>). The
# next holder starts from there: the newest base backup, then every
# segment after it, then it promotes and writes under its own epoch. The
# databases are small (node1, 2026-10: about 100M in all), so this takes
# seconds and no box keeps a live standby.
#
# - Fencing: a segment is archived only while the flag holds the epoch
#   this postgres started at. A holder that lost its lease archives
#   nothing more, and what it wrote is under its own epoch, which nobody
#   reads past the one after it.
# - The box that held the newest epoch keeps its own data when it takes
#   the lease again: it has everything that epoch wrote, archived or not.
# - Nothing is deleted: a data directory set aside for a restore is kept
#   beside it, named for when.
# - The archive's key is made by the first holder ever, and sealed to each
#   box able to run postgres (their TPM-held secrets keys,
#   fleet/boxes/<box>/hardware.json). Each holder seals it again for every
#   one of them when it starts. No secrets file holds it, and no box holds
#   another's secrets: the store sees only ciphertext.
{
  config,
  options,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.postgres.follow;
  pg = config.services.postgresql;
  data = pg.dataDir;
  state = "/var/lib/dd-pg";
  flag = "/run/dd-placed/postgresql";
  box = config.networking.hostName;
  root = "s3://${cfg.bucket}/${cfg.group}";
  common = ''
    set -euo pipefail
    export AWS_ENDPOINT=${cfg.endpoint} AWS_S3_FORCE_PATH_STYLE=true AWS_REGION=us-east-1
    export WALG_LIBSODIUM_KEY_PATH=${state}/key WALG_LIBSODIUM_KEY_TRANSFORM=hex
    export WALG_COMPRESSION_METHOD=zstd
    # the epochs the store has, oldest first
    epochs() {
      { WALG_S3_PREFIX=${root} wal-g st ls 2>/dev/null || true; } \
        | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^e[0-9]+\/?$/) { sub(/^e/, "", $i); sub(/\/$/, "", $i); print $i } }' \
        | sort -n | uniq
    }
    at() { WALG_S3_PREFIX=${root}/e$1 "''${@:2}"; }
  '';
  tools = [
    pkgs.wal-g
    pkgs.age
    pkgs.jq
    pkgs.gawk
    pkgs.gnugrep
    pkgs.coreutils
    pkgs.util-linux
  ]
  ++ cfg.agePlugins;
  recipients = lib.concatStringsSep " " (
    lib.mapAttrsToList (b: r: "${b}=${r}") (lib.filterAttrs (b: _: b != box) cfg.recipients)
    ++ lib.optional (cfg.recipients ? ${box}) "${box}=${cfg.recipients.${box}}"
  );

  # the archive's key: this box's copy, opened with its own secrets key.
  # Made only by a box that lists the archive and finds no keys in it at
  # all; one that cannot list it (the store down) does not start, so a
  # second key is never made beside the first. Only the lease's holder gets
  # here, so two boxes never make one at once. keys/fingerprint names the
  # key, and a box seals only a key that matches it
  key = ''
    if [ ! -s ${state}/key ]; then
      tmp=$(mktemp -u)
      listing=$(WALG_S3_PREFIX=${root} wal-g st ls 2>&1) || {
        echo "the store does not answer ($listing): no key, no start"
        exit 1
      }
      if ! grep -qw keys <<<"$listing"; then
        echo "no archive yet: making its key"
        head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > ${state}/key.new
        sha256sum < ${state}/key.new | cut -d' ' -f1 > "$tmp"
        WALG_S3_PREFIX=${root} wal-g st put --no-encrypt --no-compress "$tmp" keys/fingerprint
      elif WALG_S3_PREFIX=${root} wal-g st get --no-decrypt --no-decompress keys/${box}.age "$tmp" 2>/dev/null; then
        age -d -i ${cfg.identity} "$tmp" > ${state}/key.new
      else
        echo "the archive's key is not sealed to ${box} yet: a holder seals it to every box able to run postgres when it starts"
        rm -f "$tmp"
        exit 1
      fi
      rm -f "$tmp"
      chown postgres:postgres ${state}/key.new
      chmod 0400 ${state}/key.new
      mv ${state}/key.new ${state}/key
    fi
  '';

  # before postgres starts at an epoch: the data it starts from
  take = pkgs.writeShellScript "dd-pg-take" ''
    PATH=${lib.makeBinPath tools}
    ${common}
    umask 077
    mkdir -p ${state}
    chown postgres:postgres ${state}
    E=$(jq -r .epoch ${flag})
    [ "$E" -gt 0 ]
    mine=$(cat ${state}/epoch 2>/dev/null || echo 0)
    if [ "$mine" = "$E" ]; then
      echo "epoch $E: this postgres, started again"
      exit 0
    fi
    ${key}
    list=$(epochs | awk -v e="$E" '$1 < e')
    latest=$(echo "$list" | tail -n1)
    if [ -z "$latest" ] || [ "$latest" = "$mine" ]; then
      echo "epoch $E: from this box's own data (the archive's newest epoch is ''${latest:-none})"
    else
      base=""
      for e in $(echo "$list" | sort -rn); do
        # read whole, then matched: grep -q leaving early fails the pipe
        # (pipefail) when there is more than one backup to list
        backups=$(at "$e" wal-g backup-list 2>/dev/null || true)
        if [[ $backups == *base_* ]]; then base=$e; break; fi
      done
      if [ -z "$base" ]; then
        echo "the archive has epochs ($list) and no base backup: refusing to start from nothing"
        exit 1
      fi
      echo "epoch $E: restoring epoch $base's newest base backup, then every segment to epoch $latest"
      if [ -e ${data}/PG_VERSION ]; then
        aside="${data}.before-e$E.$(date +%Y%m%d%H%M%S)"
        mv ${data} "$aside"
        echo "this box's own data kept at $aside"
      fi
      mkdir -p ${data}
      chown postgres:postgres ${data}
      chmod 0700 ${data}
      at "$base" runuser -u postgres -- wal-g backup-fetch ${data} LATEST
      echo "$list" | awk -v b="$base" '$1 >= b' | sort -rn > ${state}/fetch-from
      chown postgres:postgres ${state}/fetch-from
      touch ${data}/recovery.signal
      chown postgres:postgres ${data}/recovery.signal
    fi
  '';

  # recovery: a segment, from the newest epoch that has it. A segment no
  # epoch has is the end of the archive (exit 1); one the archive has and
  # that cannot be fetched stops recovery (exit 200, which postgres takes
  # as fatal) rather than end it early, without everything after it
  fetch = pkgs.writeShellScript "dd-pg-fetch" ''
    PATH=${lib.makeBinPath tools}
    ${common}
    # restoring with no epochs to restore from is no end of the archive
    [ -s ${state}/fetch-from ] || {
      echo "no epochs to fetch $1 from: recovery stops here" >&2
      exit 200
    }
    for e in $(cat ${state}/fetch-from); do
      if at "$e" wal-g wal-fetch "$1" "$2" 2>/dev/null; then exit 0; fi
      # the archive not listed is not its end either
      has=$(at "$e" wal-g st ls wal_005/ 2>/dev/null) || {
        echo "epoch $e's archive does not answer: recovery stops here" >&2
        exit 200
      }
      if [[ $has == *"$1."* ]]; then
        for _ in 1 2 3; do
          at "$e" wal-g wal-fetch "$1" "$2" && exit 0
        done
        echo "segment $1 is in epoch $e's archive and could not be fetched: recovery stops here" >&2
        exit 200
      fi
    done
    exit 1
  '';

  # archive_command: only while the lease is still this postgres's
  archive = pkgs.writeShellScript "dd-pg-archive" ''
    PATH=${lib.makeBinPath tools}
    ${common}
    E=$(cat ${state}/epoch)
    held=$(jq -r .epoch ${flag} 2>/dev/null || echo 0)
    if [ "$held" != "$E" ]; then
      echo "not archiving $1: the lease is not this postgres's (epoch $E, flag $held)" >&2
      exit 1
    fi
    at "$E" wal-g wal-push "$1"
  '';

  # once it runs at an epoch: it is this epoch's. Nothing that needs the
  # store, which may not take writes when a box is away. Not before it has
  # replayed all it restores: postgres says it is up (read-only) as soon as
  # the base backup is consistent, and what starts after it (a base backup
  # under the new epoch, the services on its databases) must not see it
  # halfway. The epochs to fetch from stay for that replay (take writes
  # them anew for each restore)
  started = pkgs.writeShellScript "dd-pg-started" ''
    PATH=${lib.makeBinPath (tools ++ [ pg.finalPackage ])}
    set -euo pipefail
    until [ "$(runuser -u postgres -- psql -tAc 'select pg_is_in_recovery()')" = f ]; do
      sleep 1
    done
    E=$(jq -r .epoch ${flag})
    echo "$E" > ${state}/epoch.new
    chown postgres:postgres ${state}/epoch.new
    mv ${state}/epoch.new ${state}/epoch
  '';

  # then, beside it: the key sealed to every box able to hold the lease,
  # and a base backup for the next holder
  base = pkgs.writeShellScript "dd-pg-base" ''
    PATH=${lib.makeBinPath (tools ++ [ pg.finalPackage ])}
    ${common}
    pg_isready -q || { echo "postgres is not up: no base backup"; exit 0; }
    E=$(cat ${state}/epoch)
    # a seal that fails is made again at the next start; postgres runs.
    # Only the archive's own key is sealed: one this box holds that is not
    # (a fingerprint that differs) is never handed to anyone
    seal() {
    fp=$(mktemp -u)
    if ! WALG_S3_PREFIX=${root} wal-g st get --no-decrypt --no-decompress keys/fingerprint "$fp" 2>/dev/null \
      || [ "$(cat "$fp")" != "$(sha256sum < ${state}/key | cut -d' ' -f1)" ]; then
      echo "this box's key is not the archive's, or the store does not answer: sealing nothing" >&2
      rm -f "$fp"
      return
    fi
    rm -f "$fp"
    for pair in ${recipients}; do
      b=''${pair%%=*} r=''${pair#*=}
      if age -r "$r" -o ${state}/seal.$b ${state}/key \
        && WALG_S3_PREFIX=${root} wal-g st put --no-encrypt --no-compress -f ${state}/seal.$b keys/$b.age; then
        echo "the archive's key sealed to $b"
      else
        echo "could not seal the archive's key to $b" >&2
      fi
      rm -f ${state}/seal.$b
    done
    }
    seal
    # a store that takes no writes now (a box of a two-box cluster away)
    # takes this at the next run
    at "$E" wal-g backup-push ${data} || echo "no base backup this time"
  '';

  # stopping: the last segment switched and archived before postgres goes
  drain = pkgs.writeShellScript "dd-pg-drain" ''
    PATH=${
      lib.makeBinPath [
        pg.finalPackage
        pkgs.coreutils
      ]
    }
    psql -tAc "select pg_switch_wal()" >/dev/null 2>&1 || exit 0
    # done when no finished segment waits for the archiver. (Not by name: with
    # nothing written since the last switch, as right after dd-pg-base, the
    # name pg_switch_wal points at is the empty segment it started, which is
    # archived only once archive_timeout fills it, so every stop took 30s.)
    for _ in $(seq 1 60); do
      left=$(psql -tAc "select count(*) from pg_ls_archive_statusdir() where name like '%.ready'" 2>/dev/null || echo 1)
      [ "$left" = 0 ] && exit 0
      sleep 0.5
    done
    echo "$left segments not archived within 30s: the next holder restores to before them" >&2
  '';

  # handed over, stopped and its flag still up (dd-store-copy-final@postgresql
  # runs it before its copy out): every segment the drain left unarchived,
  # and the one it stopped in, which holds all it wrote since the last
  # switch. Fails when any is not archived, so the handover is asked again
  handover = pkgs.writeShellScript "dd-pg-handover" ''
    PATH=${lib.makeBinPath (tools ++ [ pg.finalPackage ])}
    set -uo pipefail
    [ ! -e ${data}/postmaster.pid ] || { echo "postgres runs: its own archiver sees to it"; exit 0; }
    ctl=$(pg_controldata ${data}) || { echo "no cluster here: nothing to archive"; exit 0; }
    cur=$(echo "$ctl" | awk -F': +' '/REDO WAL file/ { print $2 }')
    failed=0
    for r in ${data}/pg_wal/archive_status/*.ready; do
      [ -e "$r" ] || continue
      seg=$(basename "$r" .ready)
      if ${archive} ${data}/pg_wal/$seg; then
        mv "$r" "''${r%.ready}.done"
      else
        echo "$seg: not archived"
        failed=1
      fi
    done
    if [ -n "$cur" ] && [ ! -e ${data}/pg_wal/archive_status/$cur.done ]; then
      ${archive} ${data}/pg_wal/$cur || { echo "$cur: not archived"; failed=1; }
    fi
    [ "$failed" = 0 ] && echo "archived to the end, $cur"
    exit "$failed"
  '';
in
{
  options.dd.postgres.follow = {
    enable = lib.mkEnableOption "postgres archived to the store under its lease's epoch, and restored from there by the next holder";
    group = lib.mkOption {
      type = lib.types.str;
      default = "main";
      description = "the archive's name in the bucket: one postgres and the services whose databases it holds";
    };
    bucket = lib.mkOption {
      type = lib.types.str;
      default = "postgres-wal";
    };
    endpoint = lib.mkOption {
      type = lib.types.str;
      default = "http://127.0.0.1:3900";
    };
    envFile = lib.mkOption {
      type = lib.types.path;
      description = "AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY of this box's key to the bucket";
    };
    identity = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/dd/secrets-key.txt";
      description = "this box's secrets key, which opens its copy of the archive's key";
    };
    agePlugins = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ pkgs.age-plugin-tpm ];
    };
    recipients = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      description = "every box able to run postgres -> its secrets key's recipient: whom the archive's key is sealed to";
    };
  };

  config = lib.mkMerge [
    # run before the store's final copy, where there is a store
    (lib.optionalAttrs (options.dd ? store) {
      dd.store.handover.postgresql = lib.mkIf cfg.enable [ "dd-pg-handover.service" ];
    })
    (lib.mkIf cfg.enable {
      systemd.services.dd-pg-handover = {
        description = "Archive what postgres wrote last, handed over";
        unitConfig.ConditionPathExists = flag;
        restartIfChanged = false;
        serviceConfig = {
          Type = "oneshot";
          User = "postgres";
          EnvironmentFile = cfg.envFile;
          ExecStart = handover;
        };
      };
      services.postgresql.settings = {
        wal_level = "replica";
        archive_mode = "on";
        archive_command = "${archive} %p";
        # a segment at least this often, so an unplanned move loses at most this
        archive_timeout = 30;
        restore_command = "${fetch} %f %p";
        recovery_target_timeline = "latest";
      };
      systemd.services.postgresql = {
        # its key to the store is made by the store's setup, when the store
        # is this box's own (storage/store.nix)
        after = [ "dd-store-setup.service" ];
        wants = [ "dd-store-setup.service" ];
        serviceConfig = {
          EnvironmentFile = cfg.envFile;
          ExecStartPre = lib.mkBefore [ "+${take}" ];
          ExecStartPost = [ "+${started}" ];
          # the take opens the archive's key with this box's TPM-held
          # secrets key; the unit's device policy (closed) binds a + command
          # too, and age-plugin-tpm died at the open on node2 (2026-10-24)
          DeviceAllow = [ "/dev/tpmrm0 rw" ];
          # a restore is started once it has replayed the archive (started),
          # however long that takes; a writer that loses its lease meanwhile
          # is stopped by its fence (box/placed-fence.sh)
          TimeoutStartSec = "infinity";
          ExecStop = [ "${drain}" ];
          ReadWritePaths = [ state ];
        };
        environment.PGPORT = toString pg.settings.port;
      };
      systemd.services.dd-pg-base = {
        description = "A base backup of postgres for the next holder of its lease";
        after = [ "postgresql.service" ];
        wantedBy = [ "postgresql.service" ];
        # only where postgres runs: the timer finds nothing to do elsewhere
        unitConfig.ConditionPathExists = flag;
        restartIfChanged = false;
        serviceConfig = {
          Type = "oneshot";
          User = "postgres";
          EnvironmentFile = cfg.envFile;
        };
        script = "exec ${base}";
      };
      systemd.timers.dd-pg-base = {
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnActiveSec = "1d";
          OnUnitActiveSec = "1d";
        };
      };
      systemd.tmpfiles.rules = [ "d ${state} 0700 postgres postgres -" ];
    })
  ];
}
