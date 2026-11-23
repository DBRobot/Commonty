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
  address = config.dd.box.net;
  inherit (cfg) tunnel bare;
  # ssh.<domain> follows the forge like git does (needs hosts), but is a
  # proxied name only because it has its own route (door-ingress.nix)
  hosts = cfg.hosts ++ map (h: "demo-${h}") cfg.demoDoors ++ (if cfg.ssh != null then [ "ssh" ] else [ ]);
  open = cfg.enable;
}
