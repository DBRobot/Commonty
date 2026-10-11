# What the front door and the gate need of a box (types: nix/lib/needs-type.nix)
{
  nginx = {
    what = "every page's TLS and routing on a box, each request asked of the gate first";
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
    what = "the front door on the open internet: a tunnel this box opens outward";
    units = [ "cloudflared-tunnel-*" ];
    # one public front door; dd-dns points the names at it
    instances = 1;
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
    what = "the box's certificate from Let's Encrypt, for the names it serves; the DNS Worker writes its challenges";
    units = [ "dd-acme@" ]; # the helper that asks the Worker, one per challenge
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
}
