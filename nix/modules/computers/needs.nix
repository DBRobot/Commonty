# What hosting members' computers needs of a box (types: nix/lib/needs-type.nix).
# The name is placement's (place.ComputersService): a box hosts computers
# only where the release's table gives it this. Each computer is its own
# size, which its record says; this is the host's.
{
  computers = {
    what = "members' remote computers, each a VM with its own disks";
    # members' disks and memory; whose go where is the plan's
    trust = "any";
    traffic = {
      class = "high";
      upMbps = 10;
    };
    units = [
      "dd-computer@*"
      "dd-computer-prepare@*"
      "dd-computer-share@*"
      "dd-computers"
      "dd-computers-share"
      "computers-access"
      "dd-computers-usage"
      "dd-computer-facts"
    ];
    base = {
      requires = {
        arch = [ "x86_64" ]; # the guest image, for now
        kvm = true;
        disk.size.min = "20G"; # one small computer; each asks its own size of the box it lands on
      };
      uses = {
        memory = "1M";
        storage = "1M";
      };
      network = {
        outbound = "internet"; # from inside the guests
        inbound = "network"; # SSH and forwards, over the Commonty network only
      };
      depends = {
        nginx.local = true;
        dd-verify.local = true;
        dd-control.local = true; # the agreed records, and the lease each runs under
        commonty-net.local = true; # the firewall opens computers on commonty0
      };
      privacy = "plaintext"; # members' disks and memory, on the box that runs them
    };
  };
}
