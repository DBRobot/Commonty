# What members' computers need of a box (types: nix/lib/needs-type.nix).
# Each instance is its own size, which its record says; this is the host's.
{
  dd-vms = {
    what = "members' own computers, each a VM with its own disks";
    traffic = {
      class = "high";
      upMbps = 10;
    };
    units = [
      "dd-vm@*"
      "dd-vms-up"
    ];
    base = {
      requires = {
        arch = [ "x86_64" ]; # the guest image, for now
        kvm = true;
        disk.size.min = "50G";
      };
      uses = {
        memory = "1M";
        storage = "1M";
      };
      network = {
        outbound = "internet"; # from inside the guests
        inbound = "network"; # SSH and forwards, over the Commonty network only
      };
      depends.commonty-net.local = true;
      privacy = "plaintext"; # members' disks and memory, on the box that runs them
    };
  };
}
