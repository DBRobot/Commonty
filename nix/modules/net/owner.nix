# The fleet's admin administers every box from their own devices on the
# fleet's network: ssh, its dns, the cache push. The box's host - whoever has
# it at home (box.json owner) - is not its admin: their devices may ask it for
# dns (Pi-hole, where the box has it) and nothing more. The network's rules already say so
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
  host = config.dd.box.owner;
  admin = config.dd.verify.admin;
  tcp = lib.concatStringsSep "," (map toString config.dd.net.ownerPorts);
in
{
  options.dd.net.ownerPorts = lib.mkOption {
    type = lib.types.listOf lib.types.port;
    default = [
      22
      53
      80
      3900
    ];
    description = "what the fleet's admin reaches from their devices on commonty0 (tcp; 53 on udp too), beside the 443 every member has";
  };

  config = lib.mkIf (config.dd.net.enable && (admin != "" || host != null)) {
    networking.firewall.extraPackages = [ pkgs.ipset ];
    networking.firewall.extraCommands = ''
      ipset create dd-owner hash:ip -exist
      ipset create dd-owner6 hash:ip family inet6 -exist
      iptables -A nixos-fw -i commonty0 -m set --match-set dd-owner src -p tcp -m multiport --dports ${tcp} -j nixos-fw-accept
      iptables -A nixos-fw -i commonty0 -m set --match-set dd-owner src -p udp --dport 53 -j nixos-fw-accept
      ip6tables -A nixos-fw -i commonty0 -m set --match-set dd-owner6 src -p tcp -m multiport --dports ${tcp} -j nixos-fw-accept
      ip6tables -A nixos-fw -i commonty0 -m set --match-set dd-owner6 src -p udp --dport 53 -j nixos-fw-accept
      # the host's devices: dns, and nothing else of this box
      ipset create dd-host hash:ip -exist
      ipset create dd-host6 hash:ip family inet6 -exist
      for proto in tcp udp; do
        iptables -A nixos-fw -i commonty0 -m set --match-set dd-host src -p $proto --dport 53 -j nixos-fw-accept
        ip6tables -A nixos-fw -i commonty0 -m set --match-set dd-host6 src -p $proto --dport 53 -j nixos-fw-accept
      done
    '';
    systemd.services.dd-owner-devices = {
      description = "The admin's and the host's devices on the fleet's network, for the firewall";
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
      script = ''
        ${lib.optionalString (
          admin != ""
        ) "OWNER=${lib.escapeShellArg admin} SET=dd-owner ${pkgs.bash}/bin/bash ${./owner-devices.sh}"}
        ${lib.optionalString (
          host != null
        ) "OWNER=${lib.escapeShellArg host} SET=dd-host ${pkgs.bash}/bin/bash ${./owner-devices.sh}"}
      '';
      environment.SOCK = config.dd.net.socket;
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
