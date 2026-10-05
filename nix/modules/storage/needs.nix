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
}
