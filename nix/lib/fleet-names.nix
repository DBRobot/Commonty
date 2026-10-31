# The fleet's names at Cloudflare, as a gateway's config has them: `*` and
# the bare name at its address on the fleet's network, each public host a
# proxied CNAME to the tunnel while the door is open. What the DNS Worker
# keeps the zone at (servers/system/dns), and what dd-dns did before it.
config:
let
  cfg = config.dd.public;
in
{
  zone = config.dd.domain;
  joinedZone = config.dd.joinedDomain;
  address = config.dd.box.net;
  inherit (cfg) tunnel bare;
  hosts = cfg.hosts ++ map (h: "demo-${h}") cfg.demoDoors;
  open = cfg.enable;
}
