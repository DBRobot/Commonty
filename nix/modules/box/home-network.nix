# The house network: a box on the router by cable when it has one, by wifi
# when it does not. Two NetworkManager profiles, ordered by route metric, so
# the best path that is up carries the default route and the other waits.
# Every box gets a fixed address on the house network (the router's
# reservation matches), since the port forward names it.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.home;
in
{
  options.dd.home = {
    ssid = lib.mkOption {
      type = lib.types.str;
      description = "the house wifi";
    };
    pskFile = lib.mkOption {
      type = lib.types.path;
      description = "file holding the wifi password: `psk=<...>`, as NetworkManager's ensureProfiles reads secrets";
    };
    address = lib.mkOption {
      type = lib.types.str;
      description = "this box's fixed address on the house network, with prefix (192.168.1.20/24); the router reserves it";
    };
    gateway = lib.mkOption {
      type = lib.types.str;
      default = "192.168.1.1";
    };
    wired = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "the cabled adapter to the router, by mac address (an interface name would encode the usb port)";
    };
    wifi = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "the wifi interface, if any";
    };
  };

  config = {
    # A member's Wi-Fi change from the Network tab: the gate leaves it in
    # /run/dd-wifi, a root unit tries it and falls back (wifi-apply.sh), and
    # the box's link is written out each minute for the tab (wifi-status.sh)
    systemd.tmpfiles.rules = [ "d /run/dd-wifi 0770 root dd-verify -" ];
    systemd.paths.dd-wifi-apply = lib.mkIf (cfg.wifi != null) {
      wantedBy = [ "multi-user.target" ];
      pathConfig.PathChanged = "/run/dd-wifi/request";
    };
    systemd.services.dd-wifi-apply = lib.mkIf (cfg.wifi != null) {
      description = "Try a member's Wi-Fi change, falling back to the old details";
      path = [
        pkgs.networkmanager
        pkgs.jq
        pkgs.coreutils
      ];
      environment = {
        IFACE = cfg.wifi;
        DIR = "/run/dd-wifi";
      };
      serviceConfig.Type = "oneshot";
      script = builtins.readFile ./wifi-apply.sh;
      postStart = "systemctl start --no-block dd-wifi-status.service";
    };
    systemd.services.dd-wifi-status = lib.mkIf (cfg.wifi != null) {
      description = "Write how this box is on the house network";
      startAt = "minutely";
      wantedBy = [ "multi-user.target" ];
      after = [ "NetworkManager.service" ];
      path = [
        pkgs.networkmanager
        pkgs.jq
        pkgs.coreutils
        pkgs.gawk
      ];
      environment = {
        IFACE = cfg.wifi;
        DIR = "/run/dd-wifi";
      };
      serviceConfig.Type = "oneshot";
      script = builtins.readFile ./wifi-status.sh;
    };
    # the gate writes the request and reads the status and result
    systemd.services.dd-verify.serviceConfig.ReadWritePaths = [ "/run/dd-wifi" ];
    systemd.services.dd-verify.environment.VERIFY_HOUSE = "/run/dd-wifi";
    # Settings, where a Wi-Fi change is signed: every box checks the
    # signature was made there and on no other page
    systemd.services.dd-verify.environment.VERIFY_HOUSE_ORIGINS = "https://home.${config.dd.domain} https://files.${config.dd.domain}";

    networking.networkmanager.ensureProfiles = {
      environmentFiles = [ cfg.pskFile ];
      profiles =
        lib.optionalAttrs (cfg.wired != null) {
          house-wired = {
            connection = {
              id = "house-wired";
              type = "ethernet";
              autoconnect = true;
              autoconnect-priority = 100;
            };
            ethernet.mac-address = cfg.wired;
            ipv4 = {
              method = "manual";
              address1 = cfg.address;
              gateway = cfg.gateway;
              dns = "1.1.1.1;9.9.9.9;";
              route-metric = 10;
            };
            ipv6.method = "auto";
          };
        }
        // lib.optionalAttrs (cfg.wifi != null) {
          house-wifi = {
            connection = {
              id = "house-wifi";
              type = "wifi";
              interface-name = cfg.wifi;
              autoconnect = true;
              autoconnect-priority = 50;
            };
            wifi = {
              ssid = cfg.ssid;
              mode = "infrastructure";
              # the card's own address, not a random one per connection:
              # the router's reservation names it
              cloned-mac-address = "permanent";
            };
            wifi-security = {
              key-mgmt = "wpa-psk";
              psk = "$psk";
            };
            ipv4 = {
              # dhcp: two links cannot share the fixed address (two macs,
              # one ip), and the forward names the wired one. On wifi alone
              # the box is still on the tailnet and its name still follows
              # its public address
              method = "auto";
              route-metric = 20;
            };
            ipv6.method = "auto";
          };
        };
    };
    networking.networkmanager.wifi.powersave = false;
    # Two interfaces on one subnet: by default Linux answers ARP for the
    # wired address from the wifi card too, so the router learns the wifi
    # mac for it, traffic leaves by cable and returns by wifi, and every
    # long-lived connection dies within minutes (the tunnel, tailscale's
    # control poll, the runners). An address is answered for only on the
    # interface that holds it; the wifi stays a warm standby.
    boot.kernel.sysctl = {
      "net.ipv4.conf.all.arp_ignore" = 1;
      "net.ipv4.conf.all.arp_announce" = 2;
      "net.ipv4.conf.default.arp_ignore" = 1;
      "net.ipv4.conf.default.arp_announce" = 2;
    };
    # a profile that leaves the config leaves the box: ensureProfiles only
    # writes, so without this an old profile stays active until a reboot
    systemd.services.NetworkManager-ensure-profiles.preStart =
      let
        keep = lib.concatMapStringsSep " " (n: "-not -name ${lib.escapeShellArg "${n}.nmconnection"}") (
          lib.attrNames config.networking.networkmanager.ensureProfiles.profiles
        );
      in
      ''
        mkdir -p /run/NetworkManager/system-connections
        find /run/NetworkManager/system-connections -name '*.nmconnection' ${keep} -delete
      '';
  };
}
