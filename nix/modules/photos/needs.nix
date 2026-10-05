# What the photos server needs of a box (types: nix/lib/needs-type.nix)
{
  ente-museum = {
    what = "the photos server: people's encrypted photos and the albums' records";
    base = {
      requires.disk.size.min = "1G";
      # node1, 2026-10-04: museum's peak since its start; its state dir is 43K
      uses = {
        memory = "35M";
        storage = "1M";
      };
      network = {
        outbound = [ "smtp.gmail.com" ]; # sign-up and login codes
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
}
