{
  ddScript,
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.garage;
  meta = "/var/lib/garage/meta";
in
{
  # One garage cluster across every storage box. Each box holds a replica
  # of every object (replication_factor below), so losing a box loses no
  # data and reads keep working with a box down. Writes need every replica
  # up until there are three boxes: a two-box cluster that accepted writes
  # with one box away would hand out reads that miss what was just written.
  # No box is trusted, and garage does not change that: a box in the layout
  # can read and delete every object in the cluster. What it holds is
  # ciphertext (ente, the media tier and restic all encrypt before garage
  # sees anything), so the exposure is durability, and the two-copy rule is
  # the answer to that.
  options.dd.garage = {
    zone = lib.mkOption {
      type = lib.types.str;
      description = "Layout zone of this box: its region id from fleet/boxes.json.";
    };
    capacity = lib.mkOption {
      type = lib.types.str;
      description = "Capacity this box offers the layout, e.g. \"3T\".";
    };
    dataDir = lib.mkOption {
      type = lib.types.str;
      default = "/srv/garage";
      description = "Where the blocks live; a dataset the box's hardware file declares.";
    };
    publicAddr = lib.mkOption {
      type = lib.types.str;
      description = "host:port the other boxes reach this box's rpc on; its tailnet address.";
    };
    peers = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "The other storage boxes as id@host:port. Empty until a box has started once and published its id.";
    };
    replicationFactor = lib.mkOption {
      type = lib.types.int;
      default = 2;
    };
    envFile = lib.mkOption {
      type = lib.types.path;
      description = "File with GARAGE_RPC_SECRET, the same on every storage box.";
    };
    setupEnvFiles = lib.mkOption {
      type = lib.types.listOf lib.types.path;
      default = [ ];
      description = "Files with key ids and secrets the setup snippets below read.";
    };
    buckets = lib.mkOption {
      type = lib.types.attrsOf (
        lib.types.submodule {
          options = {
            key = lib.mkOption {
              type = lib.types.nullOr (
                lib.types.submodule {
                  options = {
                    name = lib.mkOption {
                      type = lib.types.str;
                      description = "the key's name in garage";
                    };
                    envPrefix = lib.mkOption {
                      type = lib.types.str;
                      description = "the key's id and secret come from <prefix>_ID and <prefix>_SECRET in one of setupEnvFiles";
                    };
                  };
                }
              );
              default = null;
              description = "a key imported and given the bucket; null: a bucket with no key of its own";
            };
            allow = lib.mkOption {
              type = lib.types.listOf (
                lib.types.enum [
                  "read"
                  "write"
                  "owner"
                ]
              );
              default = [
                "read"
                "write"
              ];
              description = "what the key may do with the bucket";
            };
            cors = lib.mkOption {
              type = lib.types.nullOr lib.types.attrs;
              default = null;
              description = "an S3 CORS configuration to put on the bucket, as the API takes it; null: none";
            };
          };
        }
      );
      default = { };
      description = "The buckets a role wants, made after the layout converges; safe to re-run. The key's material is never here: it is read from setupEnvFiles.";
    };
  };

  config = {
    services.garage = {
      enable = true;
      package = pkgs.garage; # no default; nixpkgs ships several majors
      environmentFile = cfg.envFile;
      settings = {
        metadata_dir = meta;
        data_dir = cfg.dataDir;
        db_engine = "lmdb";
        replication_factor = cfg.replicationFactor;
        consistency_mode = "consistent";
        # a daily copy of the metadata db, in ${meta}/snapshots; the backup
        # module ships it, so a cluster whose metadata is gone is rebuilt
        # from the last snapshot plus the blocks
        metadata_auto_snapshot_interval = "1d";
        # rpc on every interface; the firewall admits the tailnet and nothing
        # else, and the rpc is authenticated by the shared secret regardless
        rpc_bind_addr = "[::]:3901";
        rpc_public_addr = cfg.publicAddr;
        bootstrap_peers = cfg.peers;
        s3_api = {
          s3_region = "us-east-1"; # ente requires this string regardless of reality
          # every interface, so a box without garage can back up to this one
          # over the tailnet; the gateway proxies the public name to it
          api_bind_addr = "[::]:3900";
          root_domain = ".s3.${config.dd.domain}";
        };
        # its figures, for this box's prometheus: on loopback, and without
        # an admin token nothing else on this port answers
        admin.api_bind_addr = "127.0.0.1:3903";
      };
    };

    # The module defaults to DynamicUser, which allocates a UID at runtime - so
    # there is no stable owner for data on the pool, and garage cannot write
    # there. A static user is the normal answer for persistent data.
    users.users.garage = {
      isSystemUser = true;
      group = "garage";
      home = "/var/lib/garage";
    };
    users.groups.garage = { };

    systemd.services.garage = {
      serviceConfig = {
        DynamicUser = false;
        User = "garage";
        Group = "garage";
      };
      # the data dir is a dataset; do not start on an empty mountpoint, and
      # own it here rather than in tmpfiles, which runs before the dataset
      # is mounted and owns the directory underneath it
      unitConfig.RequiresMountsFor = [ cfg.dataDir ];
      after = [ "zfs-datasets.service" ];
      wants = [ "zfs-datasets.service" ];
      # the node key a reinstall carries in arrives owned by root
      serviceConfig.ExecStartPre = [
        "+${pkgs.coreutils}/bin/install -d -o garage -g garage -m 0750 ${cfg.dataDir}"
        "+${pkgs.coreutils}/bin/chown -R garage:garage /var/lib/garage"
      ];
    };

    systemd.tmpfiles.rules = [
      # the data dir must exist before the unit's mount namespace is set up;
      # ExecStartPre above fixes its owner once a dataset is mounted there
      "d ${cfg.dataDir} 0750 garage garage -"
      "d /var/lib/garage 0750 garage garage -"
      "d ${meta} 0700 garage garage -"
    ];

    # Converging, like zfs-datasets: safe to re-run on every rebuild and on a
    # timer, because the layout needs every box before it applies and the
    # other box may join later.
    systemd.services.garage-setup = {
      description = "Converge garage layout, buckets and keys";
      after = [ "garage.service" ];
      requires = [ "garage.service" ];
      wantedBy = [ "multi-user.target" ];
      path = [
        config.services.garage.package
        pkgs.awscli2
        pkgs.coreutils
        pkgs.gnugrep
      ];
      serviceConfig = {
        Type = "oneshot";
        EnvironmentFile = [ cfg.envFile ] ++ cfg.setupEnvFiles;
      };
      # the buckets and keys, from data: one block per bucket
      script =
        ddScript ./garage-setup.sh {
          CAPACITY = cfg.capacity;
          ZONE = cfg.zone;
        }
        + "\n"
        + lib.concatStrings (
          lib.mapAttrsToList (
            name: b:
            let
              q = lib.escapeShellArg name;
              allow = lib.concatMapStringsSep " " (a: "--${a}") (lib.unique b.allow);
              id = if b.key == null then "" else "\"$" + b.key.envPrefix + "_ID\"";
              secret = if b.key == null then "" else "\"$" + b.key.envPrefix + "_SECRET\"";
            in
            ''
              garage bucket create ${q} 2>/dev/null || true
            ''
            + lib.optionalString (b.key != null) ''
              # The secret goes in argv, and /proc/<pid>/cmdline is readable
              # by anyone on the box. Once, when the key is new, is a moment;
              # every ten minutes for the life of the fleet is a place to
              # come and look. The bucket grant takes the key id, which is
              # not a secret, so it stays where it was.
              case "$(garage key list 2>/dev/null || true)" in
                *${lib.escapeShellArg b.key.name}*) ;;
                *) garage key import ${id} ${secret} --yes -n ${lib.escapeShellArg b.key.name} 2>/dev/null || true ;;
              esac
              garage bucket allow ${allow} ${q} --key ${id} 2>/dev/null || true
            ''
            + lib.optionalString (b.cors != null && b.key != null) ''
              # garage has no cli for cors: an s3 api call with the bucket's key
              AWS_ACCESS_KEY_ID=${id} AWS_SECRET_ACCESS_KEY=${secret} AWS_DEFAULT_REGION=us-east-1 \
                aws --endpoint-url http://127.0.0.1:3900 s3api put-bucket-cors --bucket ${q} \
                --cors-configuration ${lib.escapeShellArg (builtins.toJSON b.cors)} || true
            ''
          ) cfg.buckets
        );
    };
    systemd.timers.garage-setup = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "10m";
        OnUnitActiveSec = "10m";
        RandomizedDelaySec = "1m";
      };
    };
  };
}
