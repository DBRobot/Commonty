# What the photos server needs of a box (types: nix/lib/needs-type.nix)
{
  ente-museum = {
    what = "the photos server: people's encrypted photos and the albums' records";
    # photos are encrypted on the device; it holds them and their metadata
    traffic = {
      class = "medium";
      upMbps = 10;
    };
    alwaysOn = true;
    # one library of everyone's photos: its records are one database
    instances = 1;
    # one writer: the box holding its lease, with its own keys (ente.nix)
    lease = true;
    units = [
      "ente"
      "ente-health"
    ];
    # people's photos, encrypted on their devices
    buckets.ente = { };
    # museum's api and the apps that call it, and its store's name
    hosts = [
      "api"
      "photos"
      "accounts"
      "albums"
      "cast"
      "locker"
      "photos-store"
    ];
    base = {
      requires.disk.size.min = "1G";
      # node1, 2026-10-04: museum's peak since its start; its state dir is 43K
      uses = {
        memory = "35M";
        storage = "1M";
      };
      network = {
        outbound = "none";
        inbound = "network";
      };
      depends = {
        postgresql.local = true; # its database, on the local socket
        store.local = true; # the photos, in this box's own store
        nginx.local = true;
        dd-verify.local = true; # verify-email goes through the gate
      };
      # photos are encrypted on the phone; it sees sizes, times and emails
      privacy = "metadata";
    };
  };

  ente-web = {
    what = "Ente's own web apps (photos, albums, accounts, cast, locker), with our patches, served as files";
    # Photos itself comes with the app (app/src/local.rs), but the box still
    # serves Ente's albums, accounts, cast and locker pages, which handle
    # keys in the browser: a member's box, never a stranger's, until those
    # come with the app too
    trust = "household";
    alwaysOn = true;
    # the photos app is where a member types their password: whoever has
    # root on the box that serves it could change it
    servesCode = true;
    # museum's keys with failover on: made wherever photos is carried, as
    # the gate on every such box reads the sign-in code
    units = [ "dd-ente-keys" ];
    base = {
      uses = {
        memory = "0";
        storage = "0";
      }; # nginx's; the apps are in the nix store
      network = {
        outbound = "none";
        inbound = "network";
      };
      depends = {
        nginx.local = true;
        ente-museum.local = false; # the apps call api.<domain>
      };
      privacy = "none"; # the pages; what they show is decrypted in the browser
    };
  };
}
