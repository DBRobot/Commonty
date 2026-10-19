{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.guests;
in
{
  # What a guest (a game server, a member's computer) may reach: the web,
  # and nothing of ours. A guest's traffic leaves through QEMU's user
  # network, so on the host it is qemu's own sockets, and QEMU hands the
  # guest the host's loopback at 10.0.2.2. A guest's unit runs qemu with
  # the dd-guests group beside its own, and every socket carrying that
  # group is filtered on its way out: the box's loopback and every address
  # of its own, the house's networks (its public IPv6 ones too), the
  # Commonty network, and mail are refused; DNS is not. A
  # group, not a user: the manager shares the games user and talks to the
  # gate on loopback.
  options.dd.guests = {
    enable = lib.mkEnableOption "the filter on what guests may reach";
    group = lib.mkOption {
      type = lib.types.str;
      readOnly = true;
      default = "dd-guests";
      description = "the group a guest's unit adds (SupplementaryGroups=), so its sockets are filtered";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = config.networking.firewall.enable;
        message = "guests run only behind the firewall: without it they reach the box's loopback";
      }
    ];

    users.groups.${cfg.group}.gid = 953;

    # Never taken out again, not even when the firewall stops: a guest
    # unfiltered for a moment is a guest on the box's loopback.
    networking.firewall.extraCommands = ''
      for ipt in iptables ip6tables; do
        $ipt -N dd-guests-out 2>/dev/null || true
        $ipt -F dd-guests-out
        $ipt -D OUTPUT -m owner --gid-owner ${cfg.group} --suppl-groups -j dd-guests-out 2>/dev/null || true
        $ipt -I OUTPUT 1 -m owner --gid-owner ${cfg.group} --suppl-groups -j dd-guests-out
        # replies on what was let in: a player's connection to a forwarded port
        $ipt -A dd-guests-out -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN
        $ipt -A dd-guests-out -p tcp --dport 25 -j REJECT
        # DNS, wherever the box's resolvers are: the user network's resolver
        # asks them as the box's resolv.conf says, on loopback or the house's
        $ipt -A dd-guests-out -p udp --dport 53 -j RETURN
        $ipt -A dd-guests-out -p tcp --dport 53 -j RETURN
      done
      for dst in 127.0.0.0/8 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 100.64.0.0/10 169.254.0.0/16; do
        iptables -A dd-guests-out -d $dst -j REJECT
      done
      for dst in ::1 fc00::/7 fe80::/10; do
        ip6tables -A dd-guests-out -d $dst -j REJECT
      done
      # the house's own public IPv6 networks, as this box's addresses on them
      # say now (one that appears later is caught when the firewall next
      # starts; the box's own addresses, always, just below)
      for net in $(${pkgs.iproute2}/bin/ip -6 -o addr show scope global | ${pkgs.gawk}/bin/awk '{print $4}'); do
        ip6tables -A dd-guests-out -d "$net" -j REJECT
      done
      # and every other address of this box's own, whichever network it is
      # on: a public IPv6 one reaches the box's services through lo, where
      # its firewall lets everything in
      for ipt in iptables ip6tables; do
        $ipt -A dd-guests-out -m addrtype --dst-type LOCAL -j REJECT
      done
    '';
  };
}
