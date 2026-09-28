# A box measures itself: the facts unit writes what the box is, the
# exporter serves it, prometheus on the box scrapes it. Nothing reaches
# across boxes.
{ self, ... }:
{
  name = "metrics";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1024;
  defaults.virtualisation.cores = 2; # services start in parallel instead of queueing on one thread
  defaults.virtualisation.diskSize = 2048;
  nodes.box = {
    # the gate plays no part here: without it, a change to the gate
    # is not a reason to run this again
    dd.verify.enable = false;
    imports = [
      ./box.nix
      ../modules/box/locate.nix
    ];
    dd.locate.url = "http://geo/json";
  };
  # stands in for the geoip service: answers with a fixed address and place
  nodes.geo = {
    services.nginx = {
      enable = true;
      virtualHosts.geo.locations."/json".return =
        ''200 '{"ip":"203.0.113.7","country":"US","region":"Massachusetts"}' '';
    };
    networking.firewall.enable = false;
  };
}
