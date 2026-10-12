# The box with Photos counts what each member keeps in Photos and in the
# libraries, into its heartbeat (client/cli/src/storage_ledger.rs), and
# holds each member's Photos to what the rest of their allowance leaves,
# from the fleet's count (storage/view.nix). No admin account anywhere:
# museum's own database user counts Photos, sets the limits the ledger
# works out, and lists a leaver for museum to delete (museum-own-keys.patch,
# dd_jobs.go) - each a job the box does on its own data, by the release's
# rules or a member's own leaving.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.dd.storage;
  base = config.dd.domain;
  user = "dd-storage";
  dir = "/var/lib/dd-storage";
  jobs = "/var/lib/dd-photos-jobs";
  dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  suffix = "@users.${base}";
  # as museum's own database user, peer auth on the local socket
  asEnte = {
    User = "ente";
    Group = user;
    NoNewPrivileges = true;
    ProtectSystem = "strict";
    ProtectHome = true;
    PrivateTmp = true;
  };
in
{
  options.dd.photos.demoQuota = lib.mkOption {
    type = lib.types.ints.positive;
    default = 1;
    description = "the demo's photo limit in bytes: one fits no photo, which is read-only; more, briefly, to put the demo's own photos in";
  };

  config = lib.mkIf (cfg.budgetGb != null && config.services.ente.api.enable) {
    # museum's own user, whose database it works in
    users.groups.ente = { };
    # its own, as the view's (storage/view.nix) where both run
    users.users.${user} = {
      isSystemUser = true;
      group = user;
    };
    users.groups.${user} = { };

    # It once had a database login with grants on the forge's and the
    # vault's tables; it reads their counts now (storage/usage.nix). The
    # old login goes, grants and all.
    systemd.services.dd-storage-forget-login = {
      description = "Drop the storage ledger's old database login";
      wantedBy = [ "multi-user.target" ];
      after = [ "postgresql.service" ];
      path = [ config.services.postgresql.package ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        User = "postgres";
      };
      script = ''
        [ "$(psql -tAc "select 1 from pg_roles where rolname = '${user}'")" = 1 ] || exit 0
        for db in forgejo vaultwarden ente; do
          [ "$(psql -tAc "select 1 from pg_database where datname = '$db'")" = 1 ] || continue
          psql -d "$db" -v ON_ERROR_STOP=1 -q -c 'REVOKE ALL ON ALL TABLES IN SCHEMA public FROM "${user}"' -c 'DROP OWNED BY "${user}"'
        done
        psql -v ON_ERROR_STOP=1 -q -c 'DROP ROLE "${user}"'
        echo "the ledger's database login is gone"
      '';
    };

    systemd.services.dd-storage-ledger = {
      description = "Count what each member keeps, and hold Photos to what is left";
      # its timer runs it; a switch never does
      restartIfChanged = false;
      after = [
        "dd-verify.service"
        "dd-storage-view.service"
        "dd-photos-usage.service"
      ];
      wants = [ "dd-photos-usage.service" ];
      # the limits it worked out, set by museum's own user
      unitConfig.OnSuccess = [ "dd-photos-limits.service" ];
      serviceConfig = {
        Type = "oneshot";
        User = user;
        Group = user;
        StateDirectory = "dd-storage";
        StateDirectoryMode = "0750";
        UMask = "0027";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ExecStart = lib.escapeShellArgs [
          dd
          "storage-ledger"
          "--photos-suffix"
          suffix
          "--photos-usage"
          "${jobs}/usage.tsv"
          "--photos-limits"
          "${dir}/photo-limits.tsv"
          "--budget-gb"
          (toString cfg.budgetGb)
          "--view"
          "${dir}/ledger.json"
          "--facts-out"
          "${dir}/usage.facts"
        ];
        # into this box's heartbeat, not Prometheus: as root, where the facts are
        ExecStartPost = [
          "+${pkgs.coreutils}/bin/install -m 0644 ${dir}/usage.facts /var/lib/dd-facts/usage-ledger.facts"
          "+${pkgs.coreutils}/bin/rm -f /var/lib/dd-facts/usage-ledger.prom"
        ];
      };
    };
    # what each address keeps in Photos, from museum's own tables
    systemd.services.dd-photos-usage = {
      description = "Count what each member keeps in Photos";
      restartIfChanged = false;
      after = [ "ente.service" ];
      path = [ config.services.postgresql.package ];
      serviceConfig = asEnte // {
        Type = "oneshot";
        StateDirectory = "dd-photos-jobs";
        StateDirectoryMode = "0750";
        UMask = "0027";
      };
      script = ''
        psql -d ente -v ON_ERROR_STOP=1 -tA -F $'\t' -c "
          SELECT u.dd_email, coalesce(us.storage_consumed, 0)
          FROM users u LEFT JOIN usage us USING (user_id)
          WHERE u.dd_email IS NOT NULL AND u.encrypted_email IS NOT NULL" > ${jobs}/usage.tsv.new || exit 0
        mv ${jobs}/usage.tsv.new ${jobs}/usage.tsv
      '';
    };

    # each member's limit, as the ledger worked it out from the
    # allowance (and the demo's, read-only), set where it differs
    systemd.services.dd-photos-limits = {
      description = "Hold each member's Photos to what is left of their allowance";
      restartIfChanged = false;
      after = [ "ente.service" ];
      path = [ config.services.postgresql.package ];
      serviceConfig = asEnte // {
        Type = "oneshot";
        ReadOnlyPaths = [ dir ];
      };
      script = ''
        # a file the ledger has not written for three hours (it fails, or
        # the fleet's count is stale) only ever lowers a limit, never raises
        # one; the demo is held whatever
        cp /dev/null /tmp/limits.tsv
        fresh=false
        if [ -s ${dir}/photo-limits.tsv ]; then
          cp ${dir}/photo-limits.tsv /tmp/limits.tsv
          [ $(( $(date +%s) - $(stat -c %Y ${dir}/photo-limits.tsv) )) -lt 10800 ] && fresh=true
        fi
        psql -d ente -v ON_ERROR_STOP=1 -q -v fresh="$fresh" <<'SQL'
        CREATE TEMP TABLE want (email TEXT, storage BIGINT);
        \copy want FROM '/tmp/limits.tsv'
        INSERT INTO want VALUES ('demo${suffix}', ${toString config.dd.photos.demoQuota});
        UPDATE subscriptions s
          SET storage = w.storage, original_transaction_id = 'dd-storage', product_id = 'free',
              expiry_time = 4102444800000000, payment_provider = '''
          FROM want w JOIN users u ON u.dd_email = lower(w.email)
          WHERE s.user_id = u.user_id AND u.encrypted_email IS NOT NULL AND s.storage <> w.storage
            AND (:'fresh' = 'true' OR w.storage < s.storage OR w.email = 'demo${suffix}');
        -- an account the ledger has not counted (new, or not a member's
        -- address) may keep what it has and add nothing, until it is counted
        UPDATE subscriptions s
          SET storage = GREATEST(coalesce(us.storage_consumed, 0), 1), original_transaction_id = 'dd-storage',
              product_id = 'free', expiry_time = 4102444800000000, payment_provider = '''
          FROM users u LEFT JOIN usage us USING (user_id)
          WHERE s.user_id = u.user_id AND u.encrypted_email IS NOT NULL
            AND coalesce(u.dd_email, '-') NOT IN (SELECT lower(email) FROM want)
            AND s.storage > GREATEST(coalesce(us.storage_consumed, 0), 1);
        SQL
      '';
    };

    # on its own too, whatever the ledger does: the demo, which anyone

    # may sign in to, is read-only within minutes of being made, and a

    # ledger that fails leaves no account with museum's default room

    systemd.timers.dd-photos-limits = {

      wantedBy = [ "timers.target" ];

      timerConfig = {

        OnActiveSec = "1min";

        OnUnitActiveSec = "5min";

      };

    };

    # Photos' share of deleting whoever left (docs/dev/leaving.md): the
    # gate's list of who is due - a member's own leaving, or 30 days on
    # from a removal in the release-signed member list - handed to
    # museum, which deletes the account itself (dd_jobs.go)
    systemd.services.dd-forget-photos = {
      description = "Delete the Photos accounts of members who left";
      after = [
        "dd-verify.service"
        "ente.service"
      ];
      restartIfChanged = false;
      path = [
        config.services.postgresql.package
        pkgs.curl
        pkgs.jq
      ];
      serviceConfig = asEnte // {
        Type = "oneshot";
      };
      # the gate or museum not answering yet is next time, not a failure
      script = ''
        set -uo pipefail
        list=$(curl -fsS --max-time 30 http://127.0.0.1:4181/internal/leaving) || { echo "the gate did not answer"; exit 0; }
        printf '%s' "$list" | jq -r '.members[]? | select(.due == true)
          | select((.done // []) | index("photos") | not) | .name' | while read -r name; do
          email=$(printf '%s%s' "$name" '${suffix}' | tr '[:upper:]' '[:lower:]')
          left=$(psql -d ente -tA -v e="$email" <<'SQL'
        SELECT count(*) FROM users WHERE dd_email = :'e' AND encrypted_email IS NOT NULL;
        SQL
          ) || continue
          if [ "$left" = 0 ]; then
            curl -fsS --max-time 30 -H 'content-type: application/json' \
              -d "$(jq -nc --arg n "$name" '{name: $n, service: "photos"}')" \
              http://127.0.0.1:4181/internal/leaving/done >/dev/null \
              && echo "$name: their Photos account is gone"
          else
            psql -d ente -q -v e="$email" <<'SQL'
        INSERT INTO dd_delete (email) VALUES (:'e') ON CONFLICT DO NOTHING;
        SQL
            echo "$name: their Photos account is handed to museum to delete"
          fi
        done
        exit 0
      '';
    };
    systemd.timers.dd-forget-photos = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "5min";
        # often: someone who leaves is due at once
        OnUnitActiveSec = "5min";
      };
    };

    systemd.timers.dd-storage-ledger = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        # from the timer starting, not boot: a switch that adds it starts it
        # long after boot, and OnBootSec would fire at once
        OnActiveSec = "2min";
        OnUnitActiveSec = "1h";
      };
    };

  };
}
