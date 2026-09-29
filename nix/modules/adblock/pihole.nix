# Ad blocking at home: Pi-hole on the house's main box, for the house
# network. The router hands out this box's house address as DNS (and a
# public resolver second, so the house stays online when the box is off);
# devices on the Commonty network away from home are not filtered, since
# every lookup would cross the VPN.
#
# Pi-hole keeps totals and nothing about which device looked up what
# (privacy level 3). Its api answers on this box alone and wants a password
# only the gate holds: the gate is its only door, and lets in only the
# household, signed in (box/verify/src/adblock.rs, the page and the menu's
# switch). What it asks the internet goes out encrypted, through Unbound
# over TLS to Quad9 and Cloudflare, so the line's provider cannot read it.
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
  # Unbound on the box: Pi-hole's only upstream, over TLS from here on
  unbound = "127.0.0.1#5335";
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
    passwordEnv = lib.mkOption {
      type = lib.types.path;
      description = "env file with FTLCONF_webserver_api_password: Pi-hole's api password";
    };
    passwordFile = lib.mkOption {
      type = lib.types.path;
      description = "the same password alone, for the gate";
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
      # the lists are loaded by dd-pihole-lists below, not the module's own
      # setup: that one adds them through the api every time, and the second
      # time Pi-hole answers "database_error" (the list is already there)
      lists = [ ];
      settings = {
        dns = {
          upstreams = [ unbound ];
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
        # the api, on this box alone, behind the password (passwordEnv)
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
    systemd.services.pihole-ftl = {
      serviceConfig.EnvironmentFile = [ cfg.passwordEnv ];
      after = [ "unbound.service" ];
      wants = [ "unbound.service" ];
    };

    services.unbound = {
      enable = true;
      # the box's own lookups stay as they are; this answers Pi-hole alone
      resolveLocalQueries = false;
      settings = {
        server = {
          interface = [ "127.0.0.1@5335" ];
          access-control = [ "127.0.0.0/8 allow" ];
          tls-cert-bundle = "/etc/ssl/certs/ca-certificates.crt";
          hide-identity = true;
          hide-version = true;
          qname-minimisation = true;
        };
        forward-zone = [
          {
            name = ".";
            forward-tls-upstream = true;
            forward-addr = [
              "9.9.9.9@853#dns.quad9.net"
              "149.112.112.112@853#dns.quad9.net"
              "1.1.1.1@853#cloudflare-dns.com"
              "1.0.0.1@853#cloudflare-dns.com"
            ];
          }
        ];
      };
    };

    # The lists, declared: written into Pi-hole's list table (added if new,
    # removed if no longer declared, left alone otherwise), then fetched and
    # built, then Pi-hole told to read them. Safe to run any number of times:
    # at every start, and weekly.
    systemd.services.dd-pihole-lists = {
      description = "Load and update the ad blocking lists";
      wantedBy = [ "multi-user.target" ];
      after = [
        "pihole-ftl.service"
        "network-online.target"
      ];
      wants = [ "network-online.target" ];
      requires = [ "pihole-ftl.service" ];
      path = [ pkgs.sqlite ];
      serviceConfig = {
        Type = "oneshot";
        User = config.services.pihole-ftl.user;
        Group = config.services.pihole-ftl.group;
        ExecStartPost = "+${pkgs.systemd}/bin/systemctl kill -s SIGRTMIN pihole-ftl.service";
      };
      script =
        let
          pihole = lib.getExe config.services.pihole-ftl.piholePackage;
          db = config.services.pihole-ftl.settings.files.gravity;
          sq = v: lib.replaceStrings [ "'" ] [ "''" ] v;
          addresses = lib.concatMapStringsSep ", " (l: "'${sq l.url}'") cfg.lists;
        in
        ''
          set -eu
          # a first start: gravity makes the database, empty
          [ -s ${db} ] || ${pihole} -g
          sqlite3 ${db} <<'SQL'
          ${lib.concatMapStrings (
            l:
            "INSERT OR IGNORE INTO adlist (address, enabled, comment, type) VALUES ('${sq l.url}', 1, '${sq (l.description or "")}', 0);\n"
          ) cfg.lists}DELETE FROM adlist WHERE type = 0 AND address NOT IN (${addresses});
          SQL
          ${pihole} -g
        '';
    };
    systemd.timers.dd-pihole-lists = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = "Sun 04:30";
        RandomizedDelaySec = "1h";
        Persistent = true;
      };
    };
    networking.firewall.allowedUDPPorts = [ 53 ];
    networking.firewall.allowedTCPPorts = [ 53 ];

    systemd.services.dd-verify = lib.mkIf config.dd.verify.enable {
      environment = {
        VERIFY_ADBLOCK = "http://${api}";
        VERIFY_ADBLOCK_PASSWORD_FILE = "/run/credentials/dd-verify.service/pihole-password";
        VERIFY_ADBLOCK_HOUSEHOLD = lib.concatStringsSep "," cfg.household;
        VERIFY_ADBLOCK_LAN = cfg.lan;
        # the household's on/off, kept by the gate across releases
        VERIFY_ADBLOCK_STATE = "/var/lib/dd-verify/adblock";
      };
      after = [ "pihole-ftl.service" ];
      serviceConfig.LoadCredential = [ "pihole-password:${cfg.passwordFile}" ];
    };
  };
}
