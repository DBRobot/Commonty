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
# not brought back; what it had before is set aside, never deleted. Its own
# config and secrets stay its own.
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
  ];

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
    if [ "$copied" -le "$mine" ]; then
      echo "epoch $E: from this box's own files (the copy here is of epoch $copied, these of $mine)"
      exit 0
    fi
    echo "epoch $E: restoring the copy of epoch $copied"
    rclone copyto here:forge/manifest ${state}/manifest
    # the repositories the records hold, read once postgres has replayed its
    # archive: the list is up to two minutes older, and one deleted since
    # must not come back
    q() { runuser -u postgres -- psql -d ${config.services.forgejo.database.name} -v ON_ERROR_STOP=1 -tAc "$1"; }
    [ "$(q 'select pg_is_in_recovery()')" = f ] || { echo "epoch $E: postgres is still restoring"; exit 1; }
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
    aside="${dir}/.before-e$E.$(date +%Y%m%d%H%M%S)"
    mkdir "$aside"
    for d in ${dir}/* ${dir}/.[!.]*; do
      [ -e "$d" ] || continue
      case "$d" in ${dir}/.before-e*|${dir}/custom) continue ;; esac
      mv "$d" "$aside/"
    done
    echo "what this box had kept at $aside"
    rclone copy --files-from ${state}/manifest here:forge/files ${dir}
    chown -R forgejo:forgejo ${dir}
    touch ${state}/restored
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
    zfs destroy ${dataset}@dd-copy 2>/dev/null || true
    zfs snapshot ${dataset}@dd-copy || { echo "no snapshot: not copied this time"; [ -z "$strict" ] || exit 1; exit 0; }
    src=${dir}/.zfs/snapshot/dd-copy
    # what the snapshot holds: what a restore takes, and nothing else
    rclone lsf -R --files-only --filter-from ${filters} "$src" | sort > ${state}/manifest.new
    if rclone sync --checksum --filter-from ${filters} "$src" here:forge/files \
      && rclone copyto ${state}/manifest.new here:forge/manifest \
      && echo "$E" | rclone rcat here:forge/epoch; then
      mv ${state}/manifest.new ${state}/manifest
    else
      echo "not copied this time"
      failed=1
    fi
    zfs destroy ${dataset}@dd-copy || true
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
        serviceConfig = {
          ExecStartPre = lib.mkBefore [ "+${take}" ];
          ExecStartPost = [ "+${started}" ];
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
