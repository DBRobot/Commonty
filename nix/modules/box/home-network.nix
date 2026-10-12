# The house network: a box on the router by cable when it has one, by wifi
# when it does not. Two NetworkManager profiles, ordered by route metric, so
# the best path that is up carries the default route and the other waits.
# Every box gets a fixed address on the house network (the router's
# reservation matches), since the port forward names it.
{
  config,
  lib,
  pkgs,
  ddScript,
  ...
}:
let
  cfg = config.dd.home;
  # a box with both links: the wifi is what the cable falls back on
  watch = cfg.wired != null && cfg.wifi != null;
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
    systemd.tmpfiles.rules = lib.optionals (cfg.wifi != null) [
      "d /run/dd-wifi 0770 root dd-verify -"
      # root's own files, made here and renamed into place: never created
      # in the folder the gate can write to
      "d /run/dd-wifi/root 0700 root root -"
    ];
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
    systemd.services.dd-verify.serviceConfig.ReadWritePaths = lib.mkIf (cfg.wifi != null) [
      "/run/dd-wifi"
    ];
    # a box without Wi-Fi has no folder for a change: it reports and passes
    # changes on, and no password is ever written down on it
    systemd.services.dd-verify.environment.VERIFY_HOUSE = lib.mkIf (cfg.wifi != null) "/run/dd-wifi";
    # Settings, where a Wi-Fi change is signed: every box checks the
    # signature was made there and on no other page
    systemd.services.dd-verify.environment.VERIFY_HOUSE_ORIGINS =
      "https://home.${config.dd.domain} https://files.${config.dd.domain}";

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
              # the cable's servers, not the router's: it hands out node1's
              # Pi-hole first, and a box on its wifi because node1 is slow
              # waited on every lookup
              dns = "1.1.1.1;9.9.9.9;";
              ignore-auto-dns = true;
            };
            ipv6 = {
              method = "auto";
              ignore-auto-dns = true;
            };
          };
        };
    };
    networking.networkmanager.wifi.powersave = false;
    # A cable whose adapter passes nothing with its carrier up is a link
    # NetworkManager never sees go down, and the wifi waits behind it while
    # the box is unreachable (node2, 85 minutes, 2026-10-11). The router is
    # asked over the cable every 30 s; no answer is a reset of the adapter,
    # then the wired profile down until the cable answers again
    # (wired-watch.sh). Never a reboot
    systemd.services.dd-wired-watch = lib.mkIf watch {
      description = "Check the cable to the router passes traffic, and fall back to the wifi when not";
      after = [ "NetworkManager.service" ];
      # a switch never waits on it or restarts it mid-reset
      restartIfChanged = false;
      path = [
        pkgs.networkmanager
        pkgs.iputils
        pkgs.iproute2
        pkgs.coreutils
      ];
      serviceConfig = {
        Type = "oneshot";
        RuntimeDirectory = "dd-wired-watch";
        RuntimeDirectoryPreserve = "yes";
        TimeoutStartSec = "5min";
      };
      script = ddScript ./wired-watch.sh {
        MAC = cfg.wired;
        GATEWAY = cfg.gateway;
        PROFILE = "house-wired";
        WIFI = cfg.wifi;
        DIR = "/run/dd-wired-watch";
        FACTS = "/var/lib/dd-facts";
        GRACE = 90;
        RETRY = 600;
      };
    };
    systemd.timers.dd-wired-watch = lib.mkIf watch {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        # from the timer starting, not boot: a switch that adds it starts it
        # long after boot (release 224)
        OnActiveSec = "30s";
        OnUnitActiveSec = "30s";
        AccuracySec = "1s";
      };
    };
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
