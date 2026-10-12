# A box's report on its links (box/report/nics.sh) from a real kernel and
# NetworkManager, not a made-up tree: the house's two profiles on two
# cables (a VM has no Wi-Fi card; the second stands in at the Wi-Fi's
# metric, handed DNS on node1 as the router's DHCP does), and what the
# report says of each - virtio on pci, which carries the default route,
# which waits, and whom each asks for names.
{ pkgs, self, ... }:
let
  lib = pkgs.lib;
  mac = net: n: "52:54:00:12:0${toString net}:${lib.fixedWidthString 2 "0" (toString n)}";
in
{
  name = "net-detect";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1024;
  defaults.virtualisation.vlans = [ ];
  nodes.box =
    { config, ... }:
    {
      imports = [
        ./box.nix
        ../modules/box/home-network.nix
      ];
      virtualisation.interfaces = {
        wired0 = {
          vlan = 1;
          assignIP = false;
        };
        wifi0 = {
          vlan = 2;
          assignIP = false;
        };
      };
      networking.useDHCP = false;
      networking.networkmanager.enable = true;
      networking.networkmanager.settings.main.no-auto-default = "*";
      dd.home = {
        ssid = "house";
        pskFile = pkgs.writeText "wifi.env" "psk=unused-in-the-test\n";
        address = "192.168.1.20/24";
        wired = lib.toUpper (mac 1 config.virtualisation.test.nodeNumber);
        wifi = "wifi0";
      };
      networking.networkmanager.ensureProfiles.profiles.house-wifi = lib.mkForce {
        connection = {
          id = "house-wifi";
          type = "ethernet";
          interface-name = "wifi0";
          autoconnect = true;
          autoconnect-priority = 50;
        };
        ipv4 = {
          method = "manual";
          address1 = "192.168.1.30/24";
          gateway = "192.168.1.1";
          dns = "192.168.1.21;1.1.1.1;";
          route-metric = 20;
        };
        ipv6.method = "disabled";
      };
      environment.systemPackages = [
        pkgs.jq
        pkgs.gawk
        pkgs.iw
      ];
      environment.etc."nics.sh".source = ../modules/box/report/nics.sh;
    };
}
