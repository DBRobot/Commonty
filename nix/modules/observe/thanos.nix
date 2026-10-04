{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.thanos;
  prom = config.services.prometheus;
  tsdb = "/var/lib/${prom.stateDir}/data";
  sidecar = cfg.objstoreFile != null;
  observe = cfg.sidecars != [ ];
in
{
  # The fleet-wide view, on top of the per-box one and changing nothing
  # about it. Every box keeps measuring itself; a sidecar next to its
  # prometheus ships each finished two-hour block into a bucket in the
  # cluster, labeled with the box. The observe box runs a store gateway
  # over that bucket and a query service that also asks each sidecar for
  # what is not yet in a block, so one panel spans every box and a box that
  # is gone keeps its history. Samples, labeled by who wrote them; no box
  # acts on another's numbers.
  options.dd.thanos = {
    objstoreFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Thanos objstore yaml naming the metrics bucket and this box's key; readable by prometheus. Null: no sidecar.";
    };
    sidecars = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "This box's own sidecar as host:port, for the query service; set on the observe box. Other boxes' history comes from the bucket.";
    };
    blockDuration = lib.mkOption {
      type = lib.types.str;
      default = "2h";
      description = "how often prometheus cuts a block the sidecar uploads: how long other boxes' newest history takes to reach the fleet view (a test makes it minutes)";
    };
    storeSync = lib.mkOption {
      type = lib.types.str;
      default = "3m";
      description = "how often the store looks in the bucket for new blocks";
    };
  };

  config = lib.mkMerge [
    (lib.mkIf sidecar {
      # thanos needs prometheus to cut blocks it never compacts locally
      services.prometheus = {
        globalConfig.external_labels.box = config.networking.hostName;
        extraFlags = [
          "--storage.tsdb.min-block-duration=${cfg.blockDuration}"
          "--storage.tsdb.max-block-duration=${cfg.blockDuration}"
        ];
      };
      services.thanos.sidecar = {
        enable = true;
        prometheus.url = "http://127.0.0.1:${toString prom.port}";
        tsdb.path = tsdb;
        objstore.config-file = cfg.objstoreFile;
        # this box's query alone reads it; other boxes read the bucket
        grpc-address = "127.0.0.1:10901";
        http-address = "127.0.0.1:10902";
      };
    })

    (lib.mkIf observe {
      services.thanos = {
        store = {
          enable = true;
          objstore.config-file = cfg.objstoreFile;
          grpc-address = "127.0.0.1:10905";
          http-address = "127.0.0.1:10906";
          stateDir = "thanos-store";
          sync-block-duration = cfg.storeSync;
        };
        query = {
          enable = true;
          grpc-address = "127.0.0.1:10907";
          http-address = "127.0.0.1:10903";
          endpoints = [ "127.0.0.1:10905" ] ++ cfg.sidecars;
          # a sidecar that is down is a box that is down: answer with the rest
          query.partial-response = true;
        };
        compact = {
          enable = true;
          objstore.config-file = cfg.objstoreFile;
          http-address = "127.0.0.1:10908";
          stateDir = "thanos-compact";
          retention = {
            resolution-raw = "180d";
            resolution-5m = "1y";
            resolution-1h = "5y";
          };
        };
      };
      # the objstore file is readable by prometheus; run these as it too
      # rather than as a dynamic user that cannot read it
      systemd.services = lib.genAttrs [ "thanos-store" "thanos-compact" ] (_: {
        serviceConfig = {
          DynamicUser = lib.mkForce false;
          User = "prometheus";
          Group = "prometheus";
        };
      });
    })
  ];
}
