# Postgres follows its lease (storage/postgres-follow.nix): a writes at
# epoch 1 and hands over cleanly; b takes epoch 2 from the archive with
# every row; b dies; a takes epoch 3 from what b archived, its own data set
# aside, not deleted. A postgres whose lease moved on archives nothing.
# The flags dd-control would raise are raised by hand here.
{ pkgs, self, ... }:
let
  rpc = pkgs.writeText "garage.env" "GARAGE_RPC_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
  key = box: {
    id = "GK${builtins.substring 0 24 (builtins.hashString "sha256" box)}";
    secret = builtins.hashString "sha256" "${box}-secret";
  };
  # test keys only: what each box's TPM-held secrets key is on a real one
  ages = {
    a = {
      identity = "AGE-SECRET-KEY-1RYQV6YMT4DPUURXLGDHLEP6P4QLM4NYQ4Z4E594SKSVYTXHZTF6QAJ94TJ";
      recipient = "age1gc25m409smgcklvytkksgew4ycjky4xtwvarsynwjxa9eg60jgpqfrlgp8";
    };
    b = {
      identity = "AGE-SECRET-KEY-18PLY0LS6RNEJLKPAYQRU6FD35UMDPS7WLXV94ASZ4G286NWRKSJQQPJL2V";
      recipient = "age1qg66feuknweacfxtff0mfejky0xzuk8d23zssz763lpmptxxquessgd8hd";
    };
  };
  storageBox = name: {
    dd.verify.enable = false;
    imports = [
      ./box.nix
      ../modules/storage/garage.nix
      ../modules/storage/postgres-follow.nix
    ];
    dd.garage = {
      zone = "r-test";
      capacity = "1G";
      dataDir = "/srv/garage";
      publicAddr = "${name}:3901";
      envFile = rpc;
      setupEnvFiles = [
        (pkgs.writeText "pg-key.env" ''
          GARAGE_PG_KEY_ID=${(key name).id}
          GARAGE_PG_KEY_SECRET=${(key name).secret}
        '')
      ];
      buckets.postgres-wal = {
        key = {
          name = "pg-${name}";
          envPrefix = "GARAGE_PG_KEY";
        };
        allow = [
          "read"
          "write"
        ];
      };
    };
    services.postgresql = {
      enable = true;
      ensureDatabases = [ "app" ];
    };
    # what box/placed.nix gives a placed service's units
    systemd.services.postgresql.unitConfig.ConditionPathExists = "/run/dd-placed/postgresql";
    systemd.services.postgresql-setup.unitConfig.ConditionPathExists = "/run/dd-placed/postgresql";
    dd.postgres.follow = {
      enable = true;
      envFile = pkgs.writeText "pg-s3.env" ''
        AWS_ACCESS_KEY_ID=${(key name).id}
        AWS_SECRET_ACCESS_KEY=${(key name).secret}
      '';
      identity = "/etc/age-identity";
      agePlugins = [ ];
      recipients = builtins.mapAttrs (_: k: k.recipient) ages;
    };
    environment.etc."age-identity" = {
      text = ages.${name}.identity;
      mode = "0400";
    };
    environment.systemPackages = [
      pkgs.wal-g
      pkgs.iproute2
    ];
  };
in
{
  name = "pg-follow";
  helpers = true;
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1536;
  defaults.virtualisation.cores = 2;
  defaults.virtualisation.diskSize = 4096;
  nodes = {
    a = storageBox "a";
    b = storageBox "b";
  };
  scriptEnv = { inherit rpc; };
}
