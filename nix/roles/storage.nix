{ config, ... }:
{
  # A node of the garage cluster and the datasets its pool holds. Which
  # pool, how much of it, and where the blocks live are facts of the
  # machine (fleet/boxes/<box>/box.json and fleet/boxes.json); this binds the
  # secrets: the cluster's rpc secret, shared by every storage box, and
  # this box's own backup key and password, shared with nobody.
  imports = [
    ./_sops.nix
    ../modules/storage/garage.nix
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
  sops.templates."restic.env".content = ''
    AWS_ACCESS_KEY_ID=${config.sops.placeholder.garage-backup-key-id}
    AWS_SECRET_ACCESS_KEY=${config.sops.placeholder.garage-backup-key-secret}
    AWS_DEFAULT_REGION=us-east-1
  '';
  dd.backup.envFile = config.sops.templates."restic.env".path;
  dd.backup.passwordFile = config.sops.secrets.restic-password.path;
  # this box's key to the metrics bucket, for its thanos sidecar; the
  # observe box's store and compactor use the same key of their own box
  sops.secrets.thanos-key-id = { };
  sops.secrets.thanos-key-secret = { };
  sops.templates."garage-metrics-key.env".content = ''
    GARAGE_METRICS_KEY_ID=${config.sops.placeholder.thanos-key-id}
    GARAGE_METRICS_KEY_SECRET=${config.sops.placeholder.thanos-key-secret}
  '';
  sops.templates."thanos-objstore.yaml" = {
    owner = "prometheus";
    content = ''
      type: S3
      config:
        bucket: metrics
        endpoint: 127.0.0.1:3900
        region: us-east-1
        access_key: ${config.sops.placeholder.thanos-key-id}
        secret_key: ${config.sops.placeholder.thanos-key-secret}
        insecure: true
        signature_version2: false
        bucket_lookup_type: path
    '';
  };
  dd.thanos.objstoreFile = config.sops.templates."thanos-objstore.yaml".path;

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
