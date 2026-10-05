# What the fleet's own network needs of a box (types: nix/lib/needs-type.nix)
{
  headscale = {
    what = "the network's control server: who is on it and what each may reach";
    # one control server for the fleet; its database is the network
    instances = 1;
    base = {
      # a sqlite database and its key: tiny, a floor for thousands of devices
      requires.disk.size.min = "1G";
      # node1, 2026-10-04: peak; its state (database, key, the names)
      uses = {
        memory = "18M";
        storage = "39K";
      };
      network = {
        outbound = [ "controlplane.tailscale.com" ]; # the relay map, until the fleet has relays
        inbound = "public"; # devices join through the front door
      };
      depends = {
        nginx.local = true;
        dd-verify.local = true; # mints join keys with its api key
      };
      privacy = "metadata"; # who has which devices, and when they were on
    };
  };

  commonty-net = {
    what = "this box on the fleet's network";
    everyBox = true;
    base = {
      # node1, node2, 2026-10-04: peak
      uses = {
        memory = "34M";
        storage = "18K";
      };
      network = {
        outbound = "internet"; # peers wherever they are, and Tailscale's relays
        inbound = "public"; # wireguard, udp 41642
      };
      depends.headscale.local = false;
      privacy = "none"; # carries ciphertext between peers
    };
  };

  netbridge = {
    what = "the control server's protocol through the front door, for a box that is not the control box";
    base = {
      # node2, 2026-10-04: peak
      uses = {
        memory = "13M";
        storage = "0";
      };
      network = {
        outbound = [ "headscale" ]; # headscale.<domain>, through Cloudflare
        inbound = "localhost";
      };
      depends.headscale.local = false;
      privacy = "none"; # the control protocol is encrypted end to end
    };
  };

  owner-devices = {
    what = "the box's owner's devices on the fleet's network, for the firewall's admin ports";
    everyBox = true;
    base = {
      # node1, node2, 2026-10-04: peak
      uses = {
        memory = "7M";
        storage = "0";
      };
      network = {
        outbound = "none";
        inbound = "none";
      };
      depends.commonty-net.local = true;
      privacy = "metadata"; # whose device has which address
    };
  };
}
