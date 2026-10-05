# What the storage services need of a box (types: nix/lib/needs-type.nix)
{
  postgresql = {
    what = "the databases of the services on this box";
    # one writer: a second copy is a standby (replication), not another postgres
    instances = 1;
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
        # each service's own count, read off this box's disk
        forgejo.local = true;
        vaultwarden.local = true;
      };
      privacy = "metadata"; # bytes per member, never contents
    };
  };
}
