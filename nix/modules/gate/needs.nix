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
        inbound = "tailnet"; # and the tunnel, on loopback
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
    base = {
      # node1, node2, 2026-10-04: peak; its state (sessions, replay, keys)
      uses = {
        memory = "8M";
        storage = "122K";
      };
      network = {
        outbound = "none";
        inbound = "localhost"; # nginx asks it; the network's door through nginx
      };
      depends.dd-control.local = true; # the directory comes from the agreed log
      privacy = "metadata"; # names, devices, sign-ins; never a member's files
    };
  };

  cloudflared = {
    what = "the front door on the open internet: a tunnel this box opens outward";
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
    what = "the fleet's certificate: a wildcard from Let's Encrypt, by DNS";
    base = {
      # node1, 2026-10-04: peak of a renewal; the account and certificate
      uses = {
        memory = "3M";
        storage = "263K";
      };
      network = {
        outbound = [
          "acme-v02.api.letsencrypt.org"
          "api.cloudflare.com"
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
}
