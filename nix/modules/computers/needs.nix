# What members' computers need of a box (types: nix/lib/needs-type.nix).
# Each instance is its own size, which its record says; this is the host's.
{
  dd-computers = {
    what = "members' own computers, each a VM with its own disks";
    traffic = {
      class = "high";
      upMbps = 10;
    };
    units = [
      "dd-computer@*"
      "dd-computers"
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
        commonty-net.local = true; # the firewall opens computers on commonty0
      };
      privacy = "plaintext"; # members' disks and memory, on the box that runs them
    };
  };
}
