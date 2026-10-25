# The network's control server follows its lease (needs lease;
# box/placed.nix). Who is on the network is its sqlite database, on the
# holder's disk: every minute a copy made with sqlite's own backup is
# sealed with age to each box able to hold the lease (their TPM-held
# secrets keys) and put in this box's own store (bucket headscale), which
# copies it on. A box keeping a copy that cannot hold the lease cannot read
# it. Beside it goes the epoch it was made at.
#
# A box taking the lease opens its copy when it is newer than what it has,
# with what it had set aside, never deleted. Its server's own key is its
# own: devices take the new one when they next reach it.
{
  config,
  options,
  lib,
  pkgs,
  ...
}:
let
  hcfg = config.dd.headscale;
  state = "/var/lib/headscale";
  flag = "/run/dd-placed/headscale";
  mine = "/var/lib/dd-headscale-follow";
  store = config.dd.store or { };
  env = ''
    set -a; . /var/lib/dd-store/keys/headscale.env; set +a
    export RCLONE_CONFIG=/dev/null RCLONE_CONFIG_HERE_TYPE=s3 RCLONE_CONFIG_HERE_PROVIDER=Other \
      RCLONE_CONFIG_HERE_ENDPOINT=http://127.0.0.1:${toString store.port} RCLONE_CONFIG_HERE_REGION=us-east-1 \
      RCLONE_CONFIG_HERE_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID \
      RCLONE_CONFIG_HERE_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY \
      RCLONE_CONFIG_HERE_FORCE_PATH_STYLE=true
  '';
  tools = [
    pkgs.rclone
    pkgs.jq
    pkgs.age
    pkgs.sqlite
    pkgs.coreutils
  ]
  ++ hcfg.follow.agePlugins;
  recipients = lib.concatMapStringsSep " " (r: "-r ${lib.escapeShellArg r}") (
    builtins.attrValues hcfg.follow.recipients
  );

  take = pkgs.writeShellScript "dd-headscale-take" ''
    set -euo pipefail
    PATH=${lib.makeBinPath tools}
    ${env}
    mkdir -p ${mine}
    E=$(jq -r .epoch ${flag})
    [ "$E" -gt 0 ]
    have=$(cat ${mine}/epoch 2>/dev/null || echo 0)
    [ "$have" = "$E" ] && { echo "epoch $E: this server, started again"; exit 0; }
    # no copy yet reads as nothing, not a number: none
    copied=$(rclone cat here:headscale/epoch 2>/dev/null | tr -dc 0-9 || true)
    copied=''${copied:-0}
    if [ "$copied" -le "$have" ]; then
      echo "epoch $E: from this box's own database (the copy here is of epoch $copied, this of $have)"
      exit 0
    fi
    rclone copyto here:headscale/db.sqlite.age ${mine}/db.sqlite.age
    age -d -i ${hcfg.follow.identity} -o ${mine}/db.sqlite.new ${mine}/db.sqlite.age
    if [ -e ${state}/db.sqlite ]; then
      aside="${state}/db.sqlite.before-e$E.$(date +%Y%m%d%H%M%S)"
      mkdir "$aside"
      mv ${state}/db.sqlite* "$aside/" 2>/dev/null || true
      echo "this box's own database kept at $aside"
    fi
    mv ${mine}/db.sqlite.new ${state}/db.sqlite
    chown headscale:headscale ${state}/db.sqlite
    chmod 0600 ${state}/db.sqlite
    echo "epoch $E: the network as epoch $copied left it"
  '';
  # its database, sealed, into this box's store; strict (a handover's)
  # fails when it did not copy
  copy = pkgs.writeShellScript "dd-headscale-copy" ''
    set -uo pipefail
    PATH=${lib.makeBinPath tools}
    ${env}
    strict=''${1:-}
    E=$(cat ${mine}/epoch 2>/dev/null || echo 0)
    [ "$E" -gt 0 ] || { echo "the server has not started here yet"; exit 0; }
    umask 077
    failed=0
    # headscale makes its database a few seconds after it starts, and
    # sqlite3 would make an empty one first, root's and 0600, which
    # headscale then can never open
    if [ ! -e ${state}/db.sqlite ]; then
      echo "no database here yet"
      failed=1
    elif sqlite3 ${state}/db.sqlite ".backup ${mine}/db.sqlite" \
      && age ${recipients} -o ${mine}/db.sqlite.age ${mine}/db.sqlite \
      && rclone copyto ${mine}/db.sqlite.age here:headscale/db.sqlite.age \
      && echo "$E" | rclone rcat here:headscale/epoch; then
      :
    else
      echo "not copied this time"
      failed=1
    fi
    rm -f ${mine}/db.sqlite
    [ -z "$strict" ] || exit "$failed"
    exit 0
  '';
  started = pkgs.writeShellScript "dd-headscale-started" ''
    ${pkgs.jq}/bin/jq -r .epoch ${flag} > ${mine}/epoch.new
    ${pkgs.coreutils}/bin/mv ${mine}/epoch.new ${mine}/epoch
  '';
in
{
  options.dd.headscale.follow = {
    recipients = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      description = "every box able to hold the network's lease -> its secrets key's recipient: whom the copies are sealed to";
    };
    identity = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/dd/secrets-key.txt";
    };
    agePlugins = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ pkgs.age-plugin-tpm ];
    };
  };

  config = lib.mkMerge [
    # its bucket, where a store is there to hold it (a test may have none)
    (lib.optionalAttrs (options.dd ? store) {
      dd.store.buckets.headscale = lib.mkIf (
        hcfg.enable && (config.dd.failover or false) && (store.enable or false)
      ) { };
      # handed over, its last state into the store before its final copy
      # goes out (dd-control, placed.go)
      dd.store.handover.headscale = lib.mkIf (
        hcfg.enable && (config.dd.failover or false) && (store.enable or false)
      ) [ "dd-headscale-handover.service" ];
    })
    (lib.mkIf (hcfg.enable && (config.dd.failover or false) && (store.enable or false)) {
      assertions = [
        {
          assertion = hcfg.follow.recipients != { };
          message = "the network's copies are sealed to the boxes able to hold its lease: none given";
        }
      ];
      systemd.tmpfiles.rules = [ "d ${mine} 0700 root root -" ];
      systemd.services.headscale = {
        after = [ "dd-store-setup.service" ];
        wants = [ "dd-store-setup.service" ];
        serviceConfig = {
          ExecStartPre = lib.mkBefore [ "+${take}" ];
          ExecStartPost = [ "+${started}" ];
          # the take opens the copy with this box's TPM-held secrets key;
          # the unit's device policy (closed) binds a + command too, and
          # age-plugin-tpm died at the open on node2 (2026-10-24)
          DeviceAllow = [ "/dev/tpmrm0 rw" ];
        };
      };
      systemd.services.dd-headscale-copy = {
        description = "Copy the network's database, sealed, into this box's store";
        after = [ "headscale.service" ];
        unitConfig.ConditionPathExists = flag;
        restartIfChanged = false;
        serviceConfig.Type = "oneshot";
        script = "exec ${copy}";
      };
      # handed over: the same, and it fails when the copy did not happen
      # (dd-store-copy-final@headscale runs it before its copy out)
      systemd.services.dd-headscale-handover = {
        description = "Copy the network's last database into this box's store, handed over";
        after = [ "headscale.service" ];
        unitConfig.ConditionPathExists = flag;
        restartIfChanged = false;
        serviceConfig.Type = "oneshot";
        script = "exec ${copy} strict";
      };
      systemd.timers.dd-headscale-copy = {
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnActiveSec = "1min";
          OnUnitActiveSec = "1min";
        };
      };
    })
  ];
}
