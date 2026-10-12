# What the storage services need of a box (types: nix/lib/needs-type.nix)
{
  postgresql = {
    what = "the databases of the services on this box";
    # who may run it follows the services whose databases it holds
    alwaysOn = true;
    units = [
      "postgresql"
      "postgresql-setup"
      "dd-postgres-isolation"
    ];
    # one writer, which follows its lease (storage/postgres-follow.nix):
    # the next holder restores from the archive
    instances = 1;
    lease = true;
    buckets.postgres-wal = { };
    base = {
      # every database here, and room to restore a dump beside them
      requires.disk = {
        kind = "ssd";
        size.min = "20G";
      };
      # node1, 2026-10-04: ente 22M, forgejo 70M, vaultwarden 9M
      uses = {
        memory = "178M";
        storage = "46M";
      };
      network = {
        outbound = "none";
        inbound = "localhost";
      };
      depends = { };
      privacy = "metadata";
    };
  };

  garage = {
    what = "the storage pool: buckets of ciphertext, kept on two boxes";
    # one cluster today, whose shared secret makes every peer its admin: only the owner's boxes until each box keeps its own store (box isolation)
    trust = "owner";
    traffic = {
      class = "high";
      upMbps = 50;
      downMbps = 50;
    };
    alwaysOn = true;
    spread = true;
    grows = [
      "photos"
      "libraries"
    ];
    units = [ "garage-setup" ];
    base = {
      # below this a storage box adds little; with two copies of everything
      # the smallest box caps what the pool can hold
      requires.disk.size.min = "250G";
      # node1, 2026-10-04: blocks 257G (node2 holds the same), metadata 641M
      uses = {
        memory = "516M";
        storage = "258G";
      };
      network = {
        outbound = "none";
        inbound = "network"; # S3 for the boxes, RPC between storage boxes
      };
      depends = { };
      privacy = "none"; # what it holds is encrypted before it arrives
    };
  };

  restic-backup = {
    what = "this box's own state, encrypted here, into its own bucket (two copies, on two boxes)";
    units = [
      "restic-backups-*"
      "dd-backup-stale"
      "dd-alert@*"
      "garage-meta-snapshot"
    ];
    everyBox = true;
    base = {
      # node1, 2026-10-04: peak of a run
      uses = {
        memory = "353M";
        storage = "0";
      };
      network = {
        outbound = [ "s3" ];
        inbound = "none";
      };
      depends.garage.local = false;
      privacy = "none"; # ciphertext leaves the box
    };
  };

  postgres-dumps = {
    what = "every database on this box dumped daily, on the encrypted bulk pool, and backed up";
    units = [
      "postgresqlBackup-*"
      "postgres-backup-verify"
      "postgres-backup-alert@*"
    ];
    base = {
      # a week of dumps of every database here, overestimated
      requires.disk.size.min = "10G";
      # node1, 2026-10-04: peak; the dumps
      uses = {
        memory = "50M";
        storage = "115M";
      };
      network = {
        outbound = "none";
        inbound = "none";
      };
      depends.postgresql.local = true;
      privacy = "plaintext"; # a dump is the database as it is
    };
  };

  nix-cache = {
    what = "the fleet's built closures, in the bucket, for every box's releases";
    traffic = {
      class = "high";
      downMbps = 100;
    };
    units = [
      "dd-cache-gc"
    ];
    instances = 1;
    base = {
      # node1, 2026-10-04: the nix-cache bucket (its copies are garage's)
      uses = {
        memory = "0";
        storage = "95G";
      };
      network = {
        outbound = "none";
        inbound = "network"; # boxes fetch, builders and the owner push
      };
      depends.garage.local = false;
      privacy = "none"; # what any build of this public repo makes
    };
  };

  storage-ledger = {
    what = "what each member keeps across every service, against their allowance";
    # no admin login: museum's own database user counts Photos, sets the
    # limits it works out, and lists a leaver for museum to delete
    units = [
      "dd-storage-ledger"
      "dd-storage-forget-login"
      "dd-photos-usage"
      "dd-photos-limits"
      "dd-forget-photos" # Photos' share of deleting whoever left
    ];
    instances = 1;
    base = {
      # node1, 2026-10-04: peak
      uses = {
        memory = "260M";
        storage = "13K";
      };
      network = {
        outbound = "none"; # museum's tables, on this box
        inbound = "none";
      };
      depends = {
        dd-verify.local = true; # the members and their libraries; it reads the ledger back
        storage-view.local = true; # the fleet's count, which it holds Photos to
        ente-museum.local = true; # museum's own tables; the other services' counts come in their own boxes' heartbeats
      };
      privacy = "metadata"; # bytes per member, never contents
    };
  };

  store = {
    what = "this box's own store: a garage of one with a secret nobody else has, its buckets copied to other boxes' stores (storage/store.nix)";
    # every box keeps copies for the others, and its own buckets
    everyBox = true;
    units = [
      "dd-store"
      "dd-store-setup"
      "dd-store-copy"
      "dd-store-copy-recent"
      "dd-store-copy-final@" # a writer's last writes, when dd-control hands it over
      "dd-store-facts"
      "dd-store-migrate" # the move off the old shared pool
    ];
    base = {
      # node1, 2026-10-11: a fresh garage of one
      uses = {
        memory = "100M";
        storage = "1G";
      };
      network = {
        outbound = [ "store" ]; # the other boxes' stores
        inbound = "network"; # the other boxes' copies, at store.<box>.<domain>
      };
      depends = {
        nginx.local = true;
        acme.local = true; # a certificate for store.<box>.<domain>
      };
      privacy = "none"; # what it keeps is encrypted before it arrives
    };
  };

  storage-view = {
    what = "adds up what each member keeps on every box, from the control plane's agreed heartbeats, for this box's gate (storage/view.nix)";
    units = [ "dd-storage-view" ];
    everyBox = true;
    base = {
      uses = {
        memory = "16M";
        storage = "1M";
        estimated = true; # one dd run every ten minutes, a small json
      };
      network = {
        outbound = "none";
        inbound = "none";
      };
      depends.dd-control.local = true;
      privacy = "metadata"; # each member's name and byte counts
    };
  };
}
