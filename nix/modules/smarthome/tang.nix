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
      description = "the house network: the only addresses Tang answers";
    };
  };

  config = lib.mkIf cfg.enable {
    services.tang = {
      enable = true;
      listenStream = [ (toString cfg.port) ];
      # and the box itself, where tang-show-keys reads the thumbprint
      ipAddressAllow = [
        cfg.house
        "127.0.0.1/32"
      ];
    };
    networking.firewall.extraCommands = ''
      iptables -D nixos-fw -p tcp --dport ${toString cfg.port} -s ${cfg.house} -j nixos-fw-accept 2>/dev/null || true
      iptables -I nixos-fw -p tcp --dport ${toString cfg.port} -s ${cfg.house} -j nixos-fw-accept
    '';
    networking.firewall.extraStopCommands = ''
      iptables -D nixos-fw -p tcp --dport ${toString cfg.port} -s ${cfg.house} -j nixos-fw-accept 2>/dev/null || true
    '';
  };
}
