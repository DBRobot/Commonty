# What the front door and the gate need of a box (types: nix/lib/needs-type.nix)
{
  nginx = {
    # every box has it: what it serves are the box's own names
    everyBox = true;
    what = "every page's TLS and routing on a box, each request asked of the gate first";
    # who may run it follows the services it fronts
    alwaysOn = true;
    base = {
      # node1, 2026-10-04: peak
      uses = {
        memory = "30M";
        storage = "0";
      };
      network = {
        outbound = "none";
        inbound = "network"; # and the tunnel, on loopback
      };
      depends = {
        dd-verify.local = true;
        acme.local = true; # the certificate it serves
      };
      privacy = "plaintext"; # TLS ends here
    };
  };

  dd-verify = {
    what = "the gate: sign-in, who may reach what, the directory's replica, settings pages";
    everyBox = true;
    # Pi-hole's api password, made on the box where it has Pi-hole, before
    # the gate that holds it starts (adblock/pihole.nix)
    units = [ "dd-pihole-key" ];
    base = {
      # node1, node2, 2026-10-04: peak; its state (sessions, replay, keys)
      uses = {
        memory = "8M";
        storage = "122K";
      };
      network = {
        # each where the box has that part, which is off otherwise: the
        # signed app manifest on git.<domain>, museum at api.<domain> for
        # Photos, the libraries' bucket, and the house's other boxes' gates
        # for a Wi-Fi change. On this box too, on loopback: dd-control,
        # Pi-hole, SearXNG, Thanos and Headscale
        outbound = [
          "git.<domain>"
          "api.<domain>"
          "s3"
          "the house's boxes"
        ];
        inbound = "localhost"; # nginx asks it; the network's door through nginx
      };
      depends.dd-control.local = true; # the directory comes from the agreed log
      privacy = "metadata"; # names, devices, sign-ins; never a member's files
    };
  };

  cloudflared = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    # on every box that carries it
    spread = true;
    what = "the front door on the open internet: a tunnel this box opens outward";
    # every public request passes it in the clear
    trust = "owner";
    traffic = {
      class = "high";
      upMbps = 50;
      downMbps = 50;
    };
    alwaysOn = true;
    # the one tunnel's, or with failover this box's own door and its ask
    # that the names follow what it holds (gate/public.nix ownTunnel)
    units = [
      "cloudflared-tunnel-*"
      "dd-door"
      "dd-door-names"
    ];
    # one public front door, the names pointed at it; with failover every
    # box able to be the gateway keeps its own ready, and the names follow
    # the network's lease
    base = {
      # node1, 2026-10-04: peak
      uses = {
        memory = "44M";
        storage = "0";
      };
      network = {
        outbound = [ "cloudflare" ];
        inbound = "none";
      };
      depends.nginx.local = true;
      privacy = "plaintext"; # Cloudflare ends TLS for the public hosts (README: Nothing readable by Cloudflare)
    };
  };

  acme = {
    # every box has it: what it serves are the box's own names
    everyBox = true;
    what = "the box's certificate from Let's Encrypt, for the names it serves; the DNS Worker writes its challenges";
    units = [ "dd-acme@" ]; # the helper that asks the Worker, one per challenge
    # any box's: it asks the DNS Worker for its own names alone
    alwaysOn = true;
    base = {
      # node1, 2026-10-04: peak of a renewal; the account and certificate
      uses = {
        memory = "3M";
        storage = "263K";
      };
      network = {
        outbound = [
          "acme-v02.api.letsencrypt.org"
          "cloudflare" # dns.<domain>; before `dd dns setup`, the DNS API itself
        ];
        inbound = "none";
      };
      depends = { };
      privacy = "none";
    };
  };

  dd-dns = {
    what = "the fleet's names at Cloudflare, converged at every switch";
    # where every name of the fleet points
    trust = "owner";
    alwaysOn = true;
    instances = 1;
    base = {
      uses = {
        memory = "32M";
        storage = "0";
        estimated = true; # a python oneshot, seconds long
      };
      network = {
        outbound = [ "api.cloudflare.com" ];
        inbound = "none";
      };
      depends = { };
      privacy = "none";
    };
  };

  mail-worker-check = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "checks hourly that the mail and DNS Workers Cloudflare runs are exactly the ones this release carries";
    units = [
      "dd-mail-worker-check"
      "dd-dns-worker-check"
      "dd-id-worker-check"
    ];
    base = {
      uses = {
        memory = "16M";
        storage = "0";
        estimated = true; # one fetch an hour
      };
      network = {
        outbound = [ "api.cloudflare.com" ];
        inbound = "none";
      };
      privacy = "none"; # the Worker's code, never members' mail
    };
  };
  harmonia = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "the home controllers' updates: this box's nix store, read-only, on the Commonty network (smarthome/cache-door.nix)";
    instances = 1;
    base = {
      uses = {
        memory = "20M";
        storage = "0";
        estimated = true;
      };
      network = {
        outbound = "none";
        inbound = "localhost"; # nginx's cache.<domain>
      };
      depends.nginx.local = true;
      privacy = "none"; # built from the public repository; no secret is in a store
    };
  };
}
