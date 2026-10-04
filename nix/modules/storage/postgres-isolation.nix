# One service, one database: postgres lets every login connect to every
# database unless told otherwise. Here each database admits its owner and
# whoever is granted it by name (forge/live.nix grants the gate the forge's
# events), and nobody else.
{ config, lib, ... }:
let
  pg = config.services.postgresql;
in
{
  config = lib.mkIf pg.enable {
    systemd.services.dd-postgres-isolation = {
      description = "Each database admits only its own service";
      wantedBy = [ "multi-user.target" ];
      after = [ "postgresql.service" "postgresql-setup.service" ];
      requires = [ "postgresql.service" ];
      path = [ pg.package ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        User = "postgres";
      };
      script = lib.concatMapStrings (db: ''
        psql -v ON_ERROR_STOP=1 -q -c 'REVOKE CONNECT, TEMPORARY ON DATABASE "${db}" FROM PUBLIC'
      '') pg.ensureDatabases;
    };
  };
}
