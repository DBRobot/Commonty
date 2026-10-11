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
    units = [
      "ente"
      "ente-health"
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
        garage.local = false; # the photos, wherever the storage pool keeps them
        nginx.local = true;
        dd-verify.local = true; # verify-email goes through the gate
      };
      # photos are encrypted on the phone; it sees sizes, times and emails
      privacy = "metadata";
    };
  };

  ente-web = {
    what = "Ente's own web apps (photos, albums, accounts, cast, locker), with our patches, served as files";
    # it hands browsers the code that holds people's photo keys
    trust = "owner";
    alwaysOn = true;
    # the photos app is where a member types their password: whoever has
    # root on the box that serves it could change it
    servesCode = true;
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
