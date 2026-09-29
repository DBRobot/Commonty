# Ad blocking at home: Pi-hole on the house's main box, for the house
# network. The router hands out this box's house address as DNS (and a
# public resolver second, so the house stays online when the box is off);
# devices on the Commonty network away from home are not filtered, since
# every lookup would cross the VPN.
#
# Pi-hole keeps totals and nothing about which device looked up what
# (privacy level 3). Its api answers on this box alone and asks for no
# password: the gate is its only door, and only the household may use it
# (box/verify/src/adblock.rs, the page and the menu's switch).
#
# Its dnsmasq also takes over the fleet's own names from gate/public.nix:
# every name this box serves resolves to the address on the network the
# question came in on, the Commonty network's or the house's.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.adblock;
  base = config.dd.domain;
  tailnet = config.dd.box.tailnet;
  api = "127.0.0.1:8053";
  names = lib.filter (n: lib.hasSuffix ".${base}" n || n == base) (
    builtins.attrNames config.services.nginx.virtualHosts
  );
  # both addresses for each name; localise-queries answers with the one on
  # the network the question came in on
  hosts = pkgs.writeText "commonty-names" (
    lib.concatMapStrings (n: "${tailnet} ${n}\n${cfg.lan} ${n}\n") names
  );
in
{
  options.dd.adblock = {
    enable = lib.mkEnableOption "ad blocking for this box's house network";
    lan = lib.mkOption {
      type = lib.types.str;
      description = "this box's address on the house network: what the router hands out as DNS";
    };
    household = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      description = "the members who live in this house: who may switch it and let a site through";
    };
    lists = lib.mkOption {
      type = lib.types.listOf lib.types.attrs;
      default = [
        {
          url = "https://raw.githubusercontent.com/StevenBlack/hosts/master/hosts";
          description = "StevenBlack unified hosts";
        }
      ];
      description = "the blocklists (services.pihole-ftl.lists entries)";
    };
  };

  config = lib.mkIf cfg.enable {
    services.dnsmasq.enable = lib.mkForce false;
    services.pihole-ftl = {
      enable = true;
      privacyLevel = 3;
      inherit (cfg) lists;
      settings = {
        dns = {
          upstreams = [
            "9.9.9.9"
            "149.112.112.112"
            "1.1.1.1"
          ];
          # where it listens is below, in dnsmasq's own words
          listeningMode = "NONE";
          domainNeeded = true;
          bogusPriv = true;
        };
        dhcp.active = false;
        ntp = {
          ipv4.active = false;
          ipv6.active = false;
          sync.active = false;
        };
        # the api, on this box alone; no password is set, so the gate needs none
        webserver = {
          port = api;
          api.cli_pw = true;
        };
        # the gate switches blocking and lets sites through
        misc.readOnly = false;
        misc.dnsmasq_lines = [
          "listen-address=${cfg.lan}"
          "listen-address=${tailnet}"
          # the box itself: Pi-hole's own list update checks it answers here
          "listen-address=127.0.0.1"
          "bind-dynamic"
          "localise-queries"
          "addn-hosts=${hosts}"
          # the fleet's names are answered here or not at all
          "local=/${base}/"
        ];
      };
    };
    networking.firewall.allowedUDPPorts = [ 53 ];
    networking.firewall.allowedTCPPorts = [ 53 ];

    systemd.services.dd-verify = lib.mkIf config.dd.verify.enable {
      environment = {
        VERIFY_ADBLOCK = "http://${api}";
        VERIFY_ADBLOCK_HOUSEHOLD = lib.concatStringsSep "," cfg.household;
        VERIFY_ADBLOCK_LAN = cfg.lan;
        # the household's on/off, kept by the gate across releases
        VERIFY_ADBLOCK_STATE = "/var/lib/dd-verify/adblock";
      };
      after = [ "pihole-ftl.service" ];
    };
  };
}
