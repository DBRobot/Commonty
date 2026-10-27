# The forge follows its lease (needs lease; box/placed.nix), like the
# postgres its records are in (storage/postgres-follow.nix). Its
# repositories and files are on the holder's disk: every 2 minutes they are
# copied, from a snapshot so a push is never half there, into this box's
# own store (bucket forge), which copies them on to the boxes keeping it.
# Beside them goes the list of what the snapshot held, and the epoch.
#
# A box taking the lease restores from its own store's copy when that is
# newer than what it has: only what the list names, and of the repositories
# only those its records (restored first, from postgres's archive, which is
# newer than the list can be) still hold, so a repository deleted since is
# not brought back; what it had before is set aside (the two newest of
# those kept). A restore stopped part way goes on where it was. A copy
# older than the records say was made is not taken: the box refuses the
# forge, and one with the newer copy takes it. Its own config and secrets
# stay its own.
{
  config,
  options,
  lib,
  pkgs,
  ...
}:
let
  fcfg = config.dd.forgejo;
  dir = fcfg.stateDir;
  dataset = "${config.dd.box.bulkPool}/enc/forgejo";
  flag = "/run/dd-placed/forgejo";
  state = "/var/lib/dd-forge";
  store = config.dd.store or { };
  # what is the box's own, or made again: its config and secrets, sessions,
  # queues, search indexes, scratch, and CI logs and artifacts
  skip = [
    "custom/conf/**"
    "data/sessions/**"
    "data/queues/**"
    "data/indexers/**"
    "data/tmp/**"
    "data/actions_log/**"
    "data/actions_artifacts/**"
    # what a takeover set aside (take)
    ".before-e*/**"
  ];
  filters = pkgs.writeText "dd-forge-filters" (lib.concatMapStrings (p: "- /${p}\n") skip + "+ **\n");
  env = ''
    set -a; . /var/lib/dd-store/keys/forge.env; set +a
    export RCLONE_CONFIG=/dev/null RCLONE_CONFIG_HERE_TYPE=s3 RCLONE_CONFIG_HERE_PROVIDER=Other \
      RCLONE_CONFIG_HERE_ENDPOINT=http://127.0.0.1:${toString store.port} RCLONE_CONFIG_HERE_REGION=us-east-1 \
      RCLONE_CONFIG_HERE_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID \
      RCLONE_CONFIG_HERE_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY \
      RCLONE_CONFIG_HERE_FORCE_PATH_STYLE=true
  '';
  tools = [
    pkgs.rclone
    pkgs.jq
    pkgs.coreutils
    pkgs.findutils
    pkgs.gawk
    pkgs.gnugrep
    pkgs.util-linux
    config.boot.zfs.package
    config.services.postgresql.package
    config.systemd.package
  ];

  # the records of the copies made (copy, below), in postgres beside the
  # forge's own, so a box taking over restores them with the rest: a copy
  # older than the one before the newest they name is stale
  records = "runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -tA";

  # before the forge starts at an epoch: the files it starts from
  take = pkgs.writeShellScript "dd-forge-take" ''
    set -euo pipefail
    PATH=${lib.makeBinPath tools}
    ${env}
    mkdir -p ${state}
    E=$(jq -r .epoch ${flag})
    [ "$E" -gt 0 ]
    mine=$(cat ${state}/epoch 2>/dev/null || echo 0)
    [ "$mine" = "$E" ] && { echo "epoch $E: this forge, started again"; exit 0; }
    # no copy yet reads as nothing, not a number: none
    copied=$(rclone cat here:forge/epoch 2>/dev/null | tr -dc 0-9 || true)
    copied=''${copied:-0}
    # the repositories the records hold, read once postgres has replayed its
    # archive: the list is up to two minutes older, and one deleted since
    # must not come back
    q() { runuser -u postgres -- psql -d ${config.services.forgejo.database.name} -v ON_ERROR_STOP=1 -tAc "$1"; }
    [ "$(q 'select pg_is_in_recovery()')" = f ] || { echo "epoch $E: postgres is still restoring"; exit 1; }
    need=$(${records} -d dd_follow -c "select at from forge_copies order by at desc offset 1 limit 1" 2>/dev/null || true)
    # a copy older than that: this box does not take the forge. Stopped rather
    # than tried again, so dd-control refuses it and the plan gives
    # it to a box with a newer one; nothing here is touched
    stale() {
      if [ -n "$need" ] && [ ! -e ${state}/take-older ] && [ "''${1:-0}" -lt "$need" ]; then
        echo "epoch $E: $2 is of ''${1:-no time}, older than the copy before the newest the records name ($need): refusing it (${state}/take-older takes it all the same)"
        systemctl --no-block stop forgejo.service
        exit 1
      fi
    }
    if [ "$copied" -le "$mine" ] && [ ! -e ${state}/restoring ]; then
      stale "$(awk '{ print $2 }' ${state}/stamp 2>/dev/null || true)" "this box's own last copy"
      echo "epoch $E: from this box's own files (the copy here is of epoch $copied, these of $mine)"
      rm -f ${state}/take-older
      exit 0
    fi
    stale "$(rclone cat here:forge/stamp 2>/dev/null | awk '{ print $2 }' || true)" "the copy here"
    echo "epoch $E: restoring the copy of epoch $copied"
    rclone copyto here:forge/manifest ${state}/manifest
    q "select u.lower_name || '/' || r.lower_name from repository r join \"user\" u on u.id = r.owner_id" \
      > ${state}/repos
    awk -F/ 'FILENAME == ARGV[1] { held[$0]; next }
      $1 == "repositories" && NF > 3 && $3 ~ /\.git$/ {
        n = $3; sub(/(\.wiki)?\.git$/, "", n)
        if (!(($2 "/" n) in held)) { gone[$2 "/" $3]; next }
      }
      { print }
      END { for (r in gone) print "not brought back, deleted since the list: " r > "/dev/stderr" }' \
      ${state}/repos ${state}/manifest > ${state}/manifest.held
    mv ${state}/manifest.held ${state}/manifest
    # a restore stopped part way (2026-10-25: forgejo's start timed out
    # under it every 90 s for an hour on node2, each try setting aside the
    # last and leaving files root's) goes on where it was: what this box
    # had is set aside once
    if [ -e ${state}/restoring ]; then
      echo "epoch $E: going on with a restore stopped part way; what this box had is set aside already"
    else
      aside="${dir}/.before-e$E.$(date +%Y%m%d%H%M%S)"
      mkdir "$aside"
      for d in ${dir}/* ${dir}/.[!.]*; do
        [ -e "$d" ] || continue
        case "$d" in ${dir}/.before-e*|${dir}/custom) continue ;; esac
        mv "$d" "$aside/"
      done
      echo "what this box had kept at $aside"
      echo "$E" > ${state}/restoring
      # the two newest kept
      for old in $(for d in ${dir}/.before-e*; do [ -d "$d" ] || continue; echo "''${d##*.} $d"; done | sort -n | head -n -2 | cut -d' ' -f2-); do
        echo "removing $old: two newer are kept"
        rm -rf --one-file-system "$old"
      done
    fi
    rclone copy --files-from ${state}/manifest here:forge/files ${dir}
    # the forge's own, but what it had set aside
    chown forgejo:forgejo ${dir}
    find ${dir} -mindepth 1 -maxdepth 1 ! -name '.before-e*' -exec chown -R forgejo:forgejo {} +
    touch ${state}/restored
    # what is here is the store's copy: the next copy sends what changes
    # from now on
    date +%s > ${state}/copied-at
    rm -f ${state}/restoring ${state}/take-older
  '';
  # the repositories and files, from a snapshot, into this box's store
  copy = pkgs.writeShellScript "dd-forge-copy" ''
    set -uo pipefail
    PATH=${lib.makeBinPath tools}
    ${env}
    # strict: a handover's, which fails when it did not copy
    strict=''${1:-}
    # one at a time: the timer's and a handover's share the snapshot
    exec 9>/run/dd-forge-copy.lock
    flock 9
    E=$(cat ${state}/epoch 2>/dev/null || echo 0)
    [ "$E" -gt 0 ] || { echo "the forge has not started here yet"; exit 0; }
    # only the forge started under the lease this box holds now: one that
    # has not (its take still restoring, or refused) would copy what it had
    # before over the newer copy in this store
    held=$(jq -r .epoch ${flag} 2>/dev/null || echo 0)
    [ "$held" = "$E" ] || { echo "the forge has not started under this lease here (epoch $held; it last ran at $E): nothing to copy"; exit 0; }
    # A snapshot read through .zfs stays mounted for minutes after, and is
    # busy until it is unmounted (2026-10-24, and node1's handovers of
    # 2026-10-26, which found the last one busy and took none). Each copy
    # takes one of its own, and those before it go once they can
    drop() {
      for s in $(zfs list -H -t snapshot -o name ${dataset} 2>/dev/null); do
        case "$s" in ${dataset}@dd-copy*) ;; *) continue ;; esac
        m=${dir}/.zfs/snapshot/''${s#*@}
        if grep -qF " $m " /proc/self/mounts; then umount "$m" 2>/dev/null || true; fi
        zfs destroy "$s" 2>/dev/null || echo "$s: busy; the next copy takes it away"
      done
    }
    drop
    T=$(date +%s)
    name=dd-copy-$(date +%s%N)
    zfs snapshot "${dataset}@$name" || { echo "no snapshot: not copied this time"; [ -z "$strict" ] || exit 1; exit 0; }
    src=${dir}/.zfs/snapshot/$name
    # what the snapshot holds: what a restore takes, and nothing else
    rclone lsf -R --files-only --filter-from ${filters} "$src" | sort > ${state}/manifest.new
    # What changed since the snapshot the last copy was taken from (its
    # time, less a margin), sent whatever the store has; then the rest by
    # size alone, which sends what is new and deletes what is gone, reading
    # no file. With --checksum every file was read, and a copy took 3
    # minutes (node1, 18G): no handover's copy finished within the lease's
    # time. A box with no copy of its own yet sends it all so
    since=$(cat ${state}/copied-at 2>/dev/null | tr -dc 0-9 || true)
    if [ -n "$since" ]; then
      send() {
        rclone copy --max-age "$((T - since + 120))s" --no-check-dest --filter-from ${filters} "$src" here:forge/files \
          && rclone sync --size-only --fast-list --filter-from ${filters} "$src" here:forge/files
      }
    else
      send() { rclone sync --checksum --fast-list --filter-from ${filters} "$src" here:forge/files; }
    fi
    if send \
      && rclone copyto ${state}/manifest.new here:forge/manifest \
      && echo "$E" | rclone rcat here:forge/epoch \
      && echo "$E $T" | rclone rcat here:forge/stamp; then
      mv ${state}/manifest.new ${state}/manifest
      echo "$E $T" > ${state}/stamp
      echo "$T" > ${state}/copied-at
      # and in the records: what a box taking over checks its copy against
      { [ "$(${records} -d postgres -c "select 1 from pg_database where datname = 'dd_follow'")" = 1 ] \
        || ${records} -d postgres -c "create database dd_follow" >/dev/null; } \
        && ${records} -d dd_follow -c "create table if not exists forge_copies (at bigint primary key, epoch bigint not null)" >/dev/null \
        && ${records} -d dd_follow -c "insert into forge_copies values ($T, $E) on conflict do nothing" >/dev/null \
        && ${records} -d dd_follow -c "delete from forge_copies where at not in (select at from forge_copies order by at desc limit 10)" >/dev/null \
        || echo "this copy is not in the records: a box taking over checks it against the one before"
    else
      echo "not copied this time"
      failed=1
    fi
    drop
    [ -z "$strict" ] || exit "''${failed:-0}"
    exit 0
  '';
  started = pkgs.writeShellScript "dd-forge-started" ''
    set -euo pipefail
    PATH=${lib.makeBinPath tools}
    jq -r .epoch ${flag} > ${state}/epoch.new
    mv ${state}/epoch.new ${state}/epoch
  '';
in
{
  config = lib.mkMerge [
    # its bucket, where a store is there to hold it (a test may have none)
    (lib.optionalAttrs (options.dd ? store) {
      dd.store.buckets.forge = lib.mkIf ((config.dd.failover or false) && (store.enable or false)) { };
      # handed over, the forge's last state into the store before its final
      # copy goes out (dd-control, placed.go)
      dd.store.handover.forgejo = lib.mkIf ((config.dd.failover or false) && (store.enable or false)) [
        "dd-forge-handover.service"
      ];
    })
    (lib.mkIf ((config.dd.failover or false) && (store.enable or false)) {
      systemd.tmpfiles.rules = [ "d ${state} 0700 root root -" ];
      systemd.services.forgejo = {
        # started with postgres by the plan: its role and database first
        after = [ "postgresql-setup.service" ];
        wants = [ "postgresql-setup.service" ];
        # postgres may be up after this starts: its lease or its restore can come
        # later than this one's. Retried until it is, never a failed unit
        # (2026-10-24: museum gave up after five tries and Photos stayed down)
        startLimitIntervalSec = 0;
        serviceConfig = {
          ExecStartPre = lib.mkBefore [ "+${take}" ];
          ExecStartPost = [ "+${started}" ];
          # a restore of every repository takes minutes (node1, 18G): the
          # 90 s default killed each one part way, again and again (node2,
          # 2026-10-25). A forge whose lease lapses meanwhile is stopped by
          # its fence (box/placed-fence.sh)
          TimeoutStartSec = "infinity";
          RestartSec = 2;
          RestartSteps = 5;
          RestartMaxDelaySec = 30;
        };
      };
      # its hooks are made again where they were restored: a copy keeps no
      # file's mode
      systemd.services.forgejo-admin.postStart = ''
        if [ -e ${state}/restored ]; then
          forgejo admin regenerate hooks && rm -f ${state}/restored
        fi
      '';
      systemd.services.dd-forge-copy = {
        description = "Copy the forge's repositories and files into this box's store";
        after = [ "forgejo.service" ];
        unitConfig.ConditionPathExists = flag;
        restartIfChanged = false;
        serviceConfig.Type = "oneshot";
        script = "exec ${copy}";
      };
      # handed over: the same, and it fails when the copy did not happen
      # (dd-store-copy-final@forgejo runs it before its copy out)
      systemd.services.dd-forge-handover = {
        description = "Copy the forge's last state into this box's store, handed over";
        after = [ "forgejo.service" ];
        unitConfig.ConditionPathExists = flag;
        restartIfChanged = false;
        serviceConfig.Type = "oneshot";
        script = "exec ${copy} strict";
      };
      systemd.timers.dd-forge-copy = {
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnActiveSec = "2min";
          OnUnitActiveSec = "2min";
        };
      };
    })
  ];
}
