# Ad blocking at home: Pi-hole on the house's box, a device on the house
# network and one on the Commonty network (a second test network stands in
# for it), and the household switching it through the gate. The blocklist
# is a file here: the test has no internet.
{ pkgs, self, ... }:
let
  blocklist = pkgs.writeText "blocklist" "0.0.0.0 ads.example.test\n0.0.0.0 tracker.example.test\n";
in
{
  name = "adblock";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1536;
  defaults.virtualisation.cores = 2;
  nodes.box = {
    imports = [
      ./box.nix
      ../modules/adblock/pihole.nix
    ];
    virtualisation.vlans = [
      1
      2
    ];
    dd.verify.role = pkgs.lib.mkForce "full";
    # vlan 2 stands in for the Commonty network. The test numbers machines
    # by name, so this box (away, box, house) is .2 on both
    dd.box.tailnet = "192.168.2.2";
    dd.adblock = {
      enable = true;
      lan = "192.168.1.2";
      household = [ "sarah" ];
      passwordEnv = pkgs.writeText "pihole.env" "FTLCONF_webserver_api_password=test-pihole-password\n";
      passwordFile = pkgs.writeText "pihole-password" "test-pihole-password";
      lists = [
        {
          url = "file://${blocklist}";
          description = "test list";
        }
      ];
    };
    environment.systemPackages = [
      pkgs.curl
      pkgs.jq
      pkgs.dig
      pkgs.sqlite
    ];
    # Pi-hole's list update first checks the internet is there by looking
    # these up; the test has none, and its list is a file
    networking.hosts."127.0.0.1" = [
      "raw.githubusercontent.com"
      "github.com"
    ];
  };
  nodes.house = {
    virtualisation.vlans = [ 1 ];
    environment.systemPackages = [ pkgs.dig ];
  };
  nodes.away = {
    virtualisation.vlans = [ 2 ];
    environment.systemPackages = [ pkgs.dig ];
  };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
