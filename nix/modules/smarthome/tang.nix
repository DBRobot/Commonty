# Tang on the house box, for the home controllers in its house
# (docs/dev/home-controller.md): a controller's card opens only when this
# box does one blinded key operation for it, so a Pi carried out of the
# house stays locked. The box never learns a controller's disk key.
#
# On the house network alone: never on the Commonty network, or a stolen
# Pi anywhere could ask. Its key's thumbprint goes in fleet/tang.json
# (`tang-show-keys 7654` on the box), which the controller's image carries
# so it takes this Tang and no other.
{ config, lib, ... }:
let
  cfg = config.dd.tang;
  # a box on DHCP has no fixed network to name: what is private. Those
  # sources reach a box only from its own network; the Commonty network's
  # are 100.64/10, and commonty0 forwards nothing
  private = [
    "10.0.0.0/8"
    "172.16.0.0/12"
    "192.168.0.0/16"
  ];
  houses = if cfg.house == "" then private else [ cfg.house ];
  rule =
    verb: a: "iptables ${verb} nixos-fw -p tcp --dport ${toString cfg.port} -s ${a} -j nixos-fw-accept";
in
{
  options.dd.tang = {
    enable = lib.mkEnableOption "Tang for this house's home controllers";
    port = lib.mkOption {
      type = lib.types.port;
      default = 7654;
    };
    house = lib.mkOption {
      type = lib.types.str;
      default = "192.168.1.0/24";
      description = "the house network: the only addresses Tang answers. Empty on a box with no fixed address: the private ranges";
    };
  };

  config = lib.mkIf cfg.enable {
    services.tang = {
      enable = true;
      listenStream = [ (toString cfg.port) ];
      # and the box itself, where tang-show-keys reads the thumbprint
      ipAddressAllow = houses ++ [ "127.0.0.1/32" ];
    };
    networking.firewall.extraCommands = lib.concatMapStrings (a: ''
      ${rule "-D" a} 2>/dev/null || true
      ${rule "-I" a}
    '') houses;
    networking.firewall.extraStopCommands = lib.concatMapStrings (a: ''
      ${rule "-D" a} 2>/dev/null || true
    '') houses;
  };
}
