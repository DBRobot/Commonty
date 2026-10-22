# A box's owner administers it from their own devices on the fleet's
# network: ssh, its dns (Pi-hole, where the box has it, for a device set to
# ask it), the cache push. The network's rules already say so
# (modules/net/headscale.nix); this box says it again, so a rule there that
# is wrong does not open these ports to every member. A device's
# address is not in the config, so dd-owner-devices keeps a set of them
# from this box's own view of the network: each peer and whose it is.
{
  config,
  pkgs,
  lib,
  ...
}:
let
  owner = config.dd.box.owner;
  tcp = lib.concatStringsSep "," (map toString config.dd.net.ownerPorts);
in
{
  options.dd.net.ownerPorts = lib.mkOption {
    type = lib.types.listOf lib.types.port;
    default = [
      22
      53
      80
    ];
    description = "what the box's owner reaches from their devices on commonty0 (tcp; 53 on udp too), beside the 443 every member has";
  };

  config = lib.mkIf (config.dd.net.enable && owner != null) {
    networking.firewall.extraPackages = [ pkgs.ipset ];
    networking.firewall.extraCommands = ''
      ipset create dd-owner hash:ip -exist
      ipset create dd-owner6 hash:ip family inet6 -exist
      iptables -A nixos-fw -i commonty0 -m set --match-set dd-owner src -p tcp -m multiport --dports ${tcp} -j nixos-fw-accept
      iptables -A nixos-fw -i commonty0 -m set --match-set dd-owner src -p udp --dport 53 -j nixos-fw-accept
      ip6tables -A nixos-fw -i commonty0 -m set --match-set dd-owner6 src -p tcp -m multiport --dports ${tcp} -j nixos-fw-accept
      ip6tables -A nixos-fw -i commonty0 -m set --match-set dd-owner6 src -p udp --dport 53 -j nixos-fw-accept
    '';
    systemd.services.dd-owner-devices = {
      description = "The owner's devices on the fleet's network, for the firewall";
      after = [
        "commonty-net.service"
        "firewall.service"
      ];
      path = [
        pkgs.tailscale
        pkgs.jq
        pkgs.ipset
        pkgs.gnugrep
      ];
      serviceConfig.Type = "oneshot";
      script = builtins.readFile ./owner-devices.sh;
      environment = {
        OWNER = owner;
        SOCK = config.dd.net.socket;
      };
    };
    systemd.timers.dd-owner-devices = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1min";
        OnUnitActiveSec = "1min";
      };
    };
  };
}
