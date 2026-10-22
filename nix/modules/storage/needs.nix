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
      # with failover: its base backup and its last archive on a handover,
      # for the next holder (storage/postgres-follow.nix)
      "dd-pg-base"
      "dd-pg-handover"
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

  restic-backup = {
    what = "this box's own state, encrypted here, into its own bucket in its own store, copied where the plan finds room";
    units = [
      "restic-backups-*"
      "dd-backup-stale"
      "dd-alert@*"
    ];
    everyBox = true;
    # each box's own repository, in its own store, and a copy where the
    # plan finds room (backups-<box>)
    buckets.backups.perBox = true;
    base = {
      # node1, 2026-10-04: peak of a run
      uses = {
        memory = "353M";
        storage = "0";
      };
      network = {
        outbound = [ "store" ];
        inbound = "none";
      };
      depends.store.local = true;
      privacy = "none"; # ciphertext leaves the box
    };
  };

  postgres-dumps = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
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
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "the fleet's built closures, in the bucket, for every box's releases";
    traffic = {
      class = "high";
      downMbps = 100;
    };
    units = [
      "dd-cache-gc"
    ];
    instances = 1;
    # one cache, in one box's store, which every box writes and reads with
    # its key there; nothing in it that a build cannot make again
    buckets.nix-cache = {
      copies = 1;
      shared = true;
    };
    base = {
      # node1, 2026-10-04: the nix-cache bucket
      uses = {
        memory = "0";
        storage = "95G";
      };
      network = {
        outbound = "none";
        inbound = "network"; # boxes fetch, builders and the owner push
      };
      depends = {
        store.local = true;
        # its collection keeps what the releases branch wants: the one the
        # forge holding the lease has (storage/cache.nix)
        forgejo.local = true;
      };
      privacy = "none"; # what any build of this public repo makes
    };
  };

  storage-ledger = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "what each member keeps across every service, against their allowance";
    # it holds a Photos admin login, which can set anyone's limit
    trust = "owner";
    units = [
      "dd-storage-ledger"
      "dd-storage-forget-login"
      "dd-forget-photos" # Photos' share of deleting whoever left, with the ledger's admin login
    ];
    instances = 1;
    base = {
      # node1, 2026-10-04: peak
      uses = {
        memory = "260M";
        storage = "13K";
      };
      network = {
        outbound = [ "api" ]; # museum's admin api, for photos
        inbound = "none";
      };
      depends = {
        dd-verify.local = true; # the members and their libraries; it reads the ledger back
        storage-view.local = true; # the fleet's count, which it holds Photos to
        ente-museum.local = false; # Photos' counts, from museum's api; the other services' come in their own boxes' heartbeats
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
      "dd-store-at" # keys to the shared buckets, at the stores that keep them
      "dd-store-joined" # keys for the boxes that joined on their own keys
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
