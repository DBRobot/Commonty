{ config, lib, ... }:
{
  # A node of the garage cluster and the datasets its pool holds. Which
  # pool, how much of it, and where the blocks live are facts of the
  # machine (fleet/boxes/<box>/box.json and fleet/boxes.json); this binds the
  # secrets: the cluster's rpc secret, shared by every storage box, and
  # this box's own backup key and password, shared with nobody.
  imports = [
    ./_sops.nix
    ../modules/storage/garage.nix
    ../modules/storage/snapshots.nix
  ];
  services.zfs.autoSnapshot.enable = true;
  services.zfs.autoScrub.enable = true;

  sops.secrets.garage-rpc-secret = { };
  # readable by the garage user so the CLI works for admin, not just the unit
  sops.templates."garage.env".owner = "garage";
  sops.templates."garage.env".content = ''
    GARAGE_RPC_SECRET=${config.sops.placeholder.garage-rpc-secret}
  '';
  dd.garage.envFile = config.sops.templates."garage.env".path;

  # this box's backup key: one bucket, this box's, nothing else
  sops.secrets.garage-backup-key-id = { };
  sops.secrets.garage-backup-key-secret = { };
  sops.secrets.restic-password = { };
  sops.templates."garage-backup-key.env".content = ''
    GARAGE_BACKUP_KEY_ID=${config.sops.placeholder.garage-backup-key-id}
    GARAGE_BACKUP_KEY_SECRET=${config.sops.placeholder.garage-backup-key-secret}
  '';
  # the repository is in the box's own store now, under the store's own key
  # to its bucket (storage/backup.nix); the old one stays where it was, in
  # the old pool, until that goes
  dd.backup.passwordFile = config.sops.secrets.restic-password.path;
  # this box's key to the metrics bucket, for its thanos sidecar; the
  # observe box's store and compactor use the same key of their own box
  sops.secrets.thanos-key-id = { };
  sops.secrets.thanos-key-secret = { };
  sops.templates."garage-metrics-key.env".content = ''
    GARAGE_METRICS_KEY_ID=${config.sops.placeholder.thanos-key-id}
    GARAGE_METRICS_KEY_SECRET=${config.sops.placeholder.thanos-key-secret}
  '';
  # the blocks go to the store that keeps the metrics bucket, its home's,
  # with this box's own key there (observe/thanos.nix)
  dd.thanos.fromStore = true;

  # this box's read key to the nix cache bucket: the agent fetches its
  # release closures from there. The one key that writes lives on the
  # laptop that publishes (secrets/fleet.yaml), imported into garage once
  # by hand as "cache-writer".
  sops.secrets.cache-key-id = { };
  sops.secrets.cache-key-secret = { };
  sops.templates."garage-cache-key.env".content = ''
    GARAGE_CACHE_KEY_ID=${config.sops.placeholder.cache-key-id}
    GARAGE_CACHE_KEY_SECRET=${config.sops.placeholder.cache-key-secret}
  '';
  sops.templates."aws-cache.env".content = ''
    AWS_ACCESS_KEY_ID=${config.sops.placeholder.cache-key-id}
    AWS_SECRET_ACCESS_KEY=${config.sops.placeholder.cache-key-secret}
  '';
  dd.agent.cacheEnvFile = config.sops.templates."aws-cache.env".path;

  # the metric blocks and the nix cache, copied into the store that keeps
  # each (its home) ahead of the move off the old pool. The cache could
  # start empty; on the same disk, copying it saves the first release after
  # the move from building every box again
  dd.store.migrate =
    lib.optionalAttrs (config.dd.store.buckets.metrics.home or null == config.networking.hostName) {
      metrics = {
        envFile = config.sops.templates."garage-metrics-key.env".path;
        id = "GARAGE_METRICS_KEY_ID";
        secret = "GARAGE_METRICS_KEY_SECRET";
        # thanos writes the store: once more, then no more
        once = true;
      };
    }
    //
      lib.optionalAttrs (config.dd.store.buckets.nix-cache.home or null == config.networking.hostName)
        {
          nix-cache = {
            envFile = config.sops.templates."garage-cache-key.env".path;
            id = "GARAGE_CACHE_KEY_ID";
            secret = "GARAGE_CACHE_KEY_SECRET";
          };
        };

  dd.garage.setupEnvFiles = [
    config.sops.templates."garage-backup-key.env".path
    config.sops.templates."garage-metrics-key.env".path
    config.sops.templates."garage-cache-key.env".path
  ];
  dd.garage.buckets = {
    "backups-${config.networking.hostName}" = {
      key = {
        name = "backup-${config.networking.hostName}";
        envPrefix = "GARAGE_BACKUP_KEY";
      };
      allow = [
        "read"
        "write"
        "owner"
      ];
    };
    # the metrics bucket: every box writes its own blocks; one bucket, since
    # garage scopes keys by bucket and the compactor rewrites across boxes
    metrics.key = {
      name = "metrics-${config.networking.hostName}";
      envPrefix = "GARAGE_METRICS_KEY";
    };
    # the nix cache: this box reads; only the laptop's cache-writer writes
    nix-cache = {
      key = {
        name = "cache-${config.networking.hostName}";
        envPrefix = "GARAGE_CACHE_KEY";
      };
      allow = [ "read" ];
    };
  };
}
