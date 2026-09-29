# The Git pages update as things happen (box/verify/src/forge_events.rs):
# a trigger in Forgejo's database announces each run, job and commit status
# that changes, as ids and a state and nothing else, and the gate listens.
# Forgejo has no webhook for a run or a job starting, and a webhook to this
# box would mean letting Forgejo reach its own services (webhook
# ALLOWED_HOST_LIST is "external" on purpose), so the database it is.
#
# The gate's login may connect and listen, and read no table. The trigger
# goes in after every Forgejo start, so a Forgejo upgrade that rebuilds a
# table gets it back; the forge VM test fails if it stops arriving.
{
  config,
  pkgs,
  lib,
  ...
}:
let
  db = config.services.forgejo.database.name;
  sql = pkgs.writeText "dd-forge-notify.sql" ''
    CREATE OR REPLACE FUNCTION dd_forge_run() RETURNS trigger LANGUAGE plpgsql AS $f$
    BEGIN
      PERFORM pg_notify('dd_forge', json_build_object(
        'kind', 'run', 'repo', NEW.repo_id, 'run', NEW.id, 'index', NEW.index, 'status', NEW.status)::text);
      RETURN NULL;
    END $f$;
    CREATE OR REPLACE FUNCTION dd_forge_job() RETURNS trigger LANGUAGE plpgsql AS $f$
    BEGIN
      PERFORM pg_notify('dd_forge', json_build_object(
        'kind', 'job', 'repo', NEW.repo_id, 'run', NEW.run_id, 'job', NEW.id, 'status', NEW.status)::text);
      RETURN NULL;
    END $f$;
    CREATE OR REPLACE FUNCTION dd_forge_status() RETURNS trigger LANGUAGE plpgsql AS $f$
    BEGIN
      PERFORM pg_notify('dd_forge', json_build_object(
        'kind', 'status', 'repo', NEW.repo_id, 'sha', NEW.sha, 'state', NEW.state)::text);
      RETURN NULL;
    END $f$;
    DROP TRIGGER IF EXISTS dd_forge_notify ON action_run;
    CREATE TRIGGER dd_forge_notify AFTER INSERT OR UPDATE ON action_run
      FOR EACH ROW EXECUTE FUNCTION dd_forge_run();
    DROP TRIGGER IF EXISTS dd_forge_notify ON action_run_job;
    CREATE TRIGGER dd_forge_notify AFTER INSERT OR UPDATE ON action_run_job
      FOR EACH ROW EXECUTE FUNCTION dd_forge_job();
    DROP TRIGGER IF EXISTS dd_forge_notify ON commit_status;
    CREATE TRIGGER dd_forge_notify AFTER INSERT OR UPDATE ON commit_status
      FOR EACH ROW EXECUTE FUNCTION dd_forge_status();
  '';
in
{
  config = lib.mkIf (config.services.forgejo.enable && config.services.forgejo.database.type == "postgres") {
    systemd.services.dd-forge-notify = {
      description = "The trigger that tells the Git pages what changed";
      wantedBy = [ "forgejo.service" ];
      after = [ "forgejo.service" ];
      partOf = [ "forgejo.service" ];
      path = [ config.services.postgresql.package ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        User = "postgres";
      };
      script = ''
        # the first start: Forgejo makes its tables as it comes up
        for _ in $(seq 1 120); do
          [ "$(psql -d ${db} -tAc "select to_regclass('public.commit_status') is not null and to_regclass('public.action_run_job') is not null")" = t ] && break
          sleep 1
        done
        psql -d ${db} -v ON_ERROR_STOP=1 -q -f ${sql}
      '';
    };

    # the gate listens as its own user: connect, and nothing else
    services.postgresql.ensureUsers = lib.mkIf config.dd.verify.enable [ { name = "dd-verify"; } ];
    systemd.services.dd-verify = lib.mkIf config.dd.verify.enable {
      environment.VERIFY_FORGE_EVENTS = "host=/run/postgresql user=dd-verify dbname=${db}";
      after = [ "postgresql.service" ];
      serviceConfig.RestrictAddressFamilies = [ "AF_UNIX" ];
    };
  };
}
