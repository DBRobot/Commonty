# The front door on the open internet, or not. The house line is carrier
# nat, so nothing can be forwarded to this box: the door is a Cloudflare
# tunnel this box opens outward, and only the hosts listed here are put
# behind it. Off, the fleet's names point at the gateway's address on the fleet's network
# and nothing answers an outsider at all. On, `home` and `accounts` are
# proxied names at Cloudflare that land on a listener nginx knows is the
# outside; every other host stays on the fleet's network however a name resolves,
# because nginx is told which server names take a public connection.
#
# What Cloudflare sees on the way through: the sign-up and sign-in pages,
# passkey ceremonies (a public key and a signature, nothing reusable), and
# the session for those two hosts. That is the plaintext label these two
# carry; nothing else passes there.
{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.public;
  base = config.dd.domain;
  # where the tunnel lands: an nginx listener that only cloudflared reaches
  tunnelPort = 8443;
  # every name that answers an outsider: the public hosts, and the bare
  # name when it is open too
  publicNames = map (h: "${h}.${base}") cfg.hosts ++ lib.optional cfg.bare base;
  # the demo's doors: demo-<service>.<domain>, each the service's own site
  # under a second name that outsiders may reach and that lets only the
  # demo in (the gate says who: servers/gate/src/lib.rs, demo_door and verify)
  demoParents = map (h: "${h}.${base}") cfg.demoDoors;
  demoNames = map (h: "demo-${h}.${base}") cfg.demoDoors;
  outside = publicNames ++ demoNames;
  # the same rules, as cloudflared reads them from a config of this box's own
  ingress =
    map (host: {
      hostname = host;
      service = "https://127.0.0.1:${toString tunnelPort}";
      originRequest.originServerName = host;
    }) outside
    ++ [ { service = "http_status:404"; } ];
  doorAsk = pkgs.writeShellScript "door-ask" (
    builtins.readFile ./box-sign.sh + builtins.readFile ./door-ask.sh
  );
in
{
  options.dd.public = {
    enable = lib.mkEnableOption "the front door on the open internet";
    hosts = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      description = "the subdomains that answer a connection from outside the fleet's network: the gate's own pages (sign-in, sign-up, the demo), what they need, and the network's control server. Everything else is on the fleet's network only. Lists from every module add up.";
    };
    bare = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "the bare name answers outsiders too, through the tunnel: what people type to find the site. Its server block must offer nothing a stranger may not have (roles/gateway.nix: the front page, the docs, the app download and a redirect to home).";
    };
    demoDoors = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "services whose site also answers outsiders as demo-<service>.<domain>, for the demo session alone: the demo is for anyone to look around, and the services themselves stay on the private network. Lists from every module add up.";
    };
    tunnel = lib.mkOption {
      type = lib.types.str;
      description = "the Cloudflare tunnel's id, made once through the api; its credentials are in credentialsFile";
    };
    credentialsFile = lib.mkOption {
      type = lib.types.path;
      description = "cloudflared's credentials json for the tunnel (AccountTag, TunnelSecret, TunnelID)";
    };
    ownTunnel = lib.mkOption {
      type = lib.types.bool;
      default = (config.dd.failover or false) && config.dd.dnsWorker != "";
      description = "a tunnel of this box's own, made through the DNS Worker with a secret this box makes (door-ask.sh), in place of the one tunnel and its credentials file: with failover, every box able to be the gateway has its door ready, and the names follow the gateway's lease";
    };
    tokenFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "file holding CF_DNS_API_TOKEN: dns edit and zone read on the fleet's zone; only until the DNS Worker keeps the zone (dd.dnsWorker)";
    };
  };

  config = {
    # the names follow the switch: `*` and the bare name always point at
    # this box's address on the fleet's network, the public hosts are proxied names at
    # Cloudflare while the door is open. Converged at boot and at every
    # switch; nothing here changes on its own, so no timer. Only until the
    # DNS Worker keeps them, from the release (nix/lib/fleet-names.nix)
    systemd.services.dd-dns = lib.mkIf (config.dd.dnsWorker == "") {
      description = "Converge the fleet's names at Cloudflare";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      serviceConfig = {
        Type = "oneshot";
        EnvironmentFile = cfg.tokenFile;
        DynamicUser = true;
        ExecStart = "${pkgs.python3}/bin/python3 ${./dns.py}";
      };
      environment =
        let
          names = import ../../lib/fleet-names.nix config;
        in
        {
          ZONE = names.zone;
          ADDRESS = names.address;
          TUNNEL = names.tunnel;
          HOSTS = lib.concatStringsSep " " names.hosts;
          PUBLIC = lib.boolToString names.open;
          BARE = lib.boolToString names.bare;
        };
    };

    # the door itself: an outbound tunnel, not an open port. Each public host
    # is handed to nginx's tunnel listener with its own name, so the same
    # server blocks and certificate serve both sides.
    # over tcp: quic to the edge kept dropping through the carrier's nat
    # ("no recent network activity" every minute); http2 holds
    # only where the one tunnel runs: with its own door instead, a name
    # set here alone would make an empty unit
    systemd.services."cloudflared-tunnel-${cfg.tunnel}" = lib.mkIf (cfg.enable && !cfg.ownTunnel) {
      environment.TUNNEL_TRANSPORT_PROTOCOL = "http2";
    };
    services.cloudflared = lib.mkIf (cfg.enable && !cfg.ownTunnel) {
      enable = true;
      tunnels.${cfg.tunnel} = {
        credentialsFile = cfg.credentialsFile;
        default = "http_status:404";
        ingress = lib.genAttrs outside (host: {
          service = "https://127.0.0.1:${toString tunnelPort}";
          originRequest.originServerName = host;
        });
      };
    };

    # This box's own door: its tunnel asked for once, with its own secret,
    # then run as the one above is. Up on every box able to be the gateway,
    # whether it is now or not: the names decide which one is used
    systemd.services.dd-door = lib.mkIf (cfg.enable && cfg.ownTunnel) {
      description = "This box's own tunnel to the front door";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      startLimitIntervalSec = 0;
      path = [
        pkgs.cloudflared
        pkgs.openssl
        pkgs.jq
        pkgs.curl
        pkgs.tpm2-tools
        pkgs.util-linux # flock: one user of the TPM at a time
        (import ../../lib/tpm-policy.nix pkgs)
        pkgs.coreutils
      ];
      environment = {
        BOX = config.networking.hostName;
        WORKER = "https://dns.${base}";
        KEYS = "/boot/dd/unlock";
        TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
        # over tcp: quic to the edge kept dropping through the carrier's nat
        TUNNEL_TRANSPORT_PROTOCOL = "http2";
      };
      serviceConfig = {
        StateDirectory = "dd-door";
        StateDirectoryMode = "0700";
        Restart = "always";
        RestartSec = 30;
      };
      # the tunnel asked of the DNS Worker inside the run, not before it: a
      # switch waits on everything before a service's main process, and
      # across each restart, for as long as the Worker does not answer
      script = ''
        ${doorAsk} tunnel /var/lib/dd-door
        jq -n --arg t "$(jq -r .TunnelID /var/lib/dd-door/credentials.json)" \
          --argjson ingress ${lib.escapeShellArg (builtins.toJSON ingress)} \
          '{tunnel: $t, "credentials-file": "/var/lib/dd-door/credentials.json", ingress: $ingress}' \
          > /var/lib/dd-door/config.yml
        exec cloudflared tunnel --no-autoupdate --config /var/lib/dd-door/config.yml run
      '';
    };
    # a lease taken here: the names are put in step at once, not at the
    # Worker's next minute. The flags change at every renewal; only a
    # change in which services are held here asks
    systemd.services.dd-door-names = lib.mkIf (cfg.enable && cfg.ownTunnel) {
      description = "Ask the DNS Worker to point the names at what this box now holds";
      restartIfChanged = false;
      path = [
        pkgs.openssl
        pkgs.jq
        pkgs.curl
        pkgs.tpm2-tools
        pkgs.util-linux # flock: one user of the TPM at a time
        (import ../../lib/tpm-policy.nix pkgs)
        pkgs.coreutils
      ];
      environment = {
        BOX = config.networking.hostName;
        WORKER = "https://dns.${base}";
        KEYS = "/boot/dd/unlock";
        TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
      };
      # A switch moves many flags at once, each a trigger: past systemd's
      # default limit (five starts in ten seconds) the unit failed, and the
      # first switch to release 358 counted that against the release
      startLimitIntervalSec = 0;
      serviceConfig = {
        Type = "oneshot";
        StateDirectory = "dd-door-names";
      };
      # the burst settles first, and is one ask of the Worker; the ask failing
      # (the Worker down) is never this unit failing: the Worker puts the
      # names in step itself within a minute
      script = ''
        sleep 2
        now=$(ls /run/dd-placed 2>/dev/null | grep -v '\.' | sort | tr '\n' ' ')
        [ "$now" = "$(cat /var/lib/dd-door-names/held 2>/dev/null)" ] && exit 0
        if ${doorAsk} converge; then
          echo "$now" > /var/lib/dd-door-names/held
        fi
        exit 0
      '';
    };
    systemd.paths.dd-door-names = lib.mkIf (cfg.enable && cfg.ownTunnel) {
      wantedBy = [ "paths.target" ];
      # no trigger limit: past it the path unit fails, and stops watching.
      # A run takes two seconds at least, which bounds how often it starts
      pathConfig = {
        PathChanged = "/run/dd-placed";
        TriggerLimitIntervalSec = 0;
      };
    };

    # A connection is inside only when it comes from the fleet's network and not
    # through the tunnel's listener. The map says, per connection and host,
    # whether the door is shut; every server block checks it first
    # (commonHttpConfig applies to all), so an outside connection to a
    # network-only name gets a closed connection, not a page, and nothing
    # behind the gate is reachable however the name resolves.
    services.nginx.appendHttpConfig = lib.mkIf cfg.enable ''
        # per address: a ceremony a second, ten in hand
        limit_req_zone $binary_remote_addr zone=dd_ceremony:1m rate=1r/s;
        # the directory asked with no credential at all: counted by address,
        # as a ceremony is (trying names); with one, not counted here
        map "$cookie_dd_session$http_authorization$http_x_dd_key_proof" $dd_anonymous {
          "" $binary_remote_addr;
          default "";
        }
        limit_req_zone $dd_anonymous zone=dd_directory:1m rate=1r/s;
        # and a lid on how many connections one address holds open
        limit_conn_zone $binary_remote_addr zone=dd_conn:1m;
      geo $dd_net {
        default 0;
        100.64.0.0/10 1;
        127.0.0.0/8 1;
        ::1 1;
      }
      map $server_port $dd_tunnel {
        default 0;
        ${toString tunnelPort} 1;
      }
      map "$dd_tunnel:$dd_net" $dd_inside {
        default 0;
        "0:1" 1;
      }
      map "$dd_inside:$host" $dd_shut {
        default 1;
        "~^1:" 0;
        ${lib.concatMapStringsSep "\n  " (n: ''"~^0:${lib.escapeRegex n}$" 0;'') outside}
      }
    '';
    # every host the gate stands in front of, and every public host that
    # is not one of those (the network's control server): the same check,
    # and the tunnel's listener for the public ones
    services.nginx.virtualHosts = lib.mkIf cfg.enable (
      lib.genAttrs
        (lib.unique (map (h: "${h}.${base}") config.dd.verify.hosts ++ publicNames ++ demoParents))
        (
          name:
          let
            isPublic = builtins.elem name publicNames;
            gated = builtins.elem name (map (h: "${h}.${base}") config.dd.verify.hosts);
            demoDoor = builtins.elem name demoParents;
          in
          {
            serverAliases = lib.optional demoDoor "demo-${name}";
            extraConfig = lib.mkBefore (
              ''
                if ($dd_shut) { return 444; }
              ''
              + lib.optionalString isPublic ''
                # the tunnel's side of this host
                listen 127.0.0.1:${toString tunnelPort} ssl;
                # a visitor who typed http: Cloudflare carries it in as-is
                if ($http_x_forwarded_proto = "http") { return 301 https://$host$request_uri; }
                # the visitor's own address, as Cloudflare reports it, so the
                # limits below count visitors and not the tunnel
                set_real_ip_from 127.0.0.1;
                real_ip_header CF-Connecting-IP;
                # what a public connection may send: pages and json, never a
                # photo (uploads go to services that are not public); and a lid
                # on how many connections one address holds open
                client_max_body_size 1m;
                client_body_timeout 15s;
                client_header_timeout 15s;
                # requests in flight: a page of pictures opens thirty at once
                limit_conn dd_conn 64;
                # anything at all, per visitor (the zone: modules/gate/verify.nix)
                limit_req zone=dd_visitor burst=500 nodelay;
                limit_req_status 429;
              ''
              # a demo door is the service's own site, which members use for
              # uploads and streams: only the tunnel and the visitor's address
              # here; the demo itself is read-only and paced at the gate
              + lib.optionalString (demoDoor && !isPublic) ''
                listen 127.0.0.1:${toString tunnelPort} ssl;
                if ($http_x_forwarded_proto = "http") { return 301 https://$host$request_uri; }
                set_real_ip_from 127.0.0.1;
                real_ip_header CF-Connecting-IP;
              ''
            );
          }
          // lib.optionalAttrs (isPublic && gated) {
            # the ceremonies a stranger may start: sign-up, sign-in, enrol, a
            # device joining the network, a device being added by code.
            # A person clicks these a few times; a script gets told to wait.
            # The rest of /_dd/ wants a credential first
            locations."~ ^/_dd/(join|login|enrol|network|add)/" = {
              proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}";
              extraConfig = ''
                limit_req zone=dd_ceremony burst=10 nodelay;
                limit_req_status 429;
                proxy_set_header X-Original-URI $request_uri;
              '';
            };
            # the directory and invites: a stranger asks a few times, a
            # script trying names is told to wait (servers/gate directory.rs)
            locations."~ ^/_dd/(directory|invite)(/|$)" = {
              proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}";
              extraConfig = ''
                limit_req zone=dd_directory burst=10 nodelay;
                limit_req zone=dd_user burst=300 nodelay;
                limit_req zone=dd_visitor burst=500 nodelay;
                limit_req_status 429;
                proxy_set_header X-Original-URI $request_uri;
              '';
            };
          }
        )
    );
  };
}
