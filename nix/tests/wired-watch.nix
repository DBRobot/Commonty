# A box with a cable and a wifi to the house router, and the cable going
# quiet with its carrier up, as node2's USB adapter did (2026-10-11). Two
# test networks are the two links, bridged on the router as a house router's
# switch and access point are; the wifi profile is a second cable here, at
# the wifi's metric, since a VM has no wifi card. The watchdog finds the
# cable dead, has no USB device to reset, takes the wired profile down, and
# brings it back once the cable answers again.
{ pkgs, self, ... }:
let
  lib = pkgs.lib;
  # the test's mac for a machine on a network (nixos/lib/qemu-common.nix)
  mac = net: n: "52:54:00:12:0${toString net}:${lib.fixedWidthString 2 "0" (toString n)}";
in
{
  name = "wired-watch";
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
      # no "Wired connection 1" on either link: the house's profiles only
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
          route-metric = 20;
        };
        ipv6.method = "disabled";
      };
      environment.systemPackages = [ pkgs.iproute2 ];
    };
  nodes.router = {
    virtualisation.interfaces = {
      lan0 = {
        vlan = 1;
        assignIP = false;
      };
      ap0 = {
        vlan = 2;
        assignIP = false;
      };
    };
    networking.useDHCP = false;
    networking.firewall.enable = false;
    networking.bridges.br0.interfaces = [
      "lan0"
      "ap0"
    ];
    networking.interfaces.br0.ipv4.addresses = [
      {
        address = "192.168.1.1";
        prefixLength = 24;
      }
    ];
  };
}
