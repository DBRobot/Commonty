# Ad blocking at home: Pi-hole on the house's main box, for the house
# network. The router hands out this box's house address as DNS (and a
# public resolver second, so the house stays online when the box is off).
# Devices on the Commonty network are handed Quad9, unfiltered
# (net/headscale.nix), not this: every lookup would cross the network. It
# listens on the box's address there too, for the box's owner's own devices
# alone (net/owner.nix), which are filtered only if set to ask it.
#
# Pi-hole keeps totals and nothing about which device looked up what
# (privacy level 3). Its api answers on this box alone and wants a password
# only the gate holds: the gate is its only door, and lets in only the
# household, signed in (servers/gate/src/adblock.rs, the page and the menu's
# switch). What it asks the internet goes out encrypted, through Unbound
# over TLS to Quad9 and Cloudflare, so the line's provider cannot read it.
#
# The fleet's own names it does not answer itself: they resolve as the
# world answers them (below, at dnsmasq_lines).
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.adblock;
  api = "127.0.0.1:8053";
  # the house network, from this box's fixed address on it (192.168.1.20/24)
  house = config.dd.home.address;
  # Unbound on the box: Pi-hole's only upstream, over TLS from here on
  unbound = "127.0.0.1#5335";
  # Pi-hole's api password, made on this box once and kept (dd-pihole-key):
  # Pi-hole and the gate here hold it, nobody else. No secrets file has it,
  # so a box that takes the house's DNS over makes its own
  keys = "/var/lib/dd-pihole-key";
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
      default = "${keys}/env";
      description = "env file with FTLCONF_webserver_api_password: Pi-hole's api password; by default the one this box made";
    };
    passwordFile = lib.mkOption {
      type = lib.types.path;
      default = "${keys}/password";
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

  imports = [ ../box/remote-jobs.nix ];

  config = lib.mkIf cfg.enable {
    services.dnsmasq.enable = lib.mkForce false;
    services.pihole-ftl = {
      enable = true;
      privacyLevel = 3;
      # no record of which names a device looked up, not even dnsmasq's log
      settings.dns.queryLogging = false;
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
        ]
        # the box's owner's devices, on the fleet's network, when set to ask
        # it: the network hands every device Quad9, and the firewall lets in
        # the owner's alone (net/owner.nix)
        ++ lib.optional (config.dd.box.net != "") "listen-address=${config.dd.box.net}"
        ++ [
          # the box itself: Pi-hole's own list update checks it answers here
          "listen-address=127.0.0.1"
          "bind-dynamic"
          # The fleet's own names are answered as the world answers them, not
          # here: the box takes web traffic from the fleet's network and the
          # tunnel, never the house network, so a house address for them was
          # a page that never loaded. Public ones go through the tunnel; the
          # rest name the box's address on the fleet's network, reached with
          # the app's network on.
        ];
      };
    };
    # the gate's and Pi-hole's password, before either starts; the gate's
    # (gate/needs.nix dd-verify), so it is there wherever the gate is
    systemd.services.dd-pihole-key = {
      description = "Make Pi-hole's api password on this box";
      wantedBy = [
        "pihole-ftl.service"
        "dd-verify.service"
      ];
      before = [
        "pihole-ftl.service"
        "dd-verify.service"
      ];
      path = [
        pkgs.openssl
        pkgs.coreutils
      ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        set -eu
        umask 077
        install -d -m 0700 ${keys}
        [ -s ${keys}/password ] || { openssl rand -hex 24 > ${keys}/password.new && mv ${keys}/password.new ${keys}/password; }
        want="FTLCONF_webserver_api_password=$(cat ${keys}/password)"
        [ "$(cat ${keys}/env 2>/dev/null)" = "$want" ] || { printf '%s\n' "$want" > ${keys}/env.new && mv ${keys}/env.new ${keys}/env; }
      '';
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
            ];
          }
        ];
      };
    };

    # The lists, declared: written into Pi-hole's list table (added if new,
    # removed if no longer declared, left alone otherwise), then fetched and
    # built, then Pi-hole told to read them. Safe to run any number of times:
    # at every start, and weekly.
    # gravity fetches every list from the internet, and a fetch that stalls
    # has no end: after a switch, never in one (box/remote-jobs.nix)
    dd.remoteJobs.dd-pihole-lists = { };
    systemd.services.dd-pihole-lists = {
      description = "Load and update the ad blocking lists";
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
          # Pi-hole may be writing the file right now: wait for it, not fail
          sqlite3 ${db} <<'SQL'
          .timeout 30000
          ${
            lib.concatMapStrings (
              l:
              "INSERT OR IGNORE INTO adlist (address, enabled, comment, type) VALUES ('${sq l.url}', 1, '${sq (l.description or "")}', 0);\n"
            ) cfg.lists
          }DELETE FROM adlist WHERE type = 0 AND address NOT IN (${addresses});
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
    # The house's own devices and the owner's own devices, and nobody else:
    # not the rest of the Commonty network, where other households are.
    # From the house network by its address, never on commonty0 whatever
    # the source says; on commonty0 the owner's devices alone (roles/core.nix,
    # the dd-owner set).
    networking.firewall.extraCommands = ''
      for proto in udp tcp; do
        iptables -D nixos-fw ! -i commonty0 -s ${house} -p $proto --dport 53 -j nixos-fw-accept 2>/dev/null || true
        iptables -A nixos-fw ! -i commonty0 -s ${house} -p $proto --dport 53 -j nixos-fw-accept
      done
    '';

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
