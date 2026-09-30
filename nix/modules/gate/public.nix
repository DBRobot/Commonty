# The front door on the open internet, or not. The house line is carrier
# nat, so nothing can be forwarded to this box: the door is a Cloudflare
# tunnel this box opens outward, and only the hosts listed here are put
# behind it. Off, the fleet's names point at the gateway's tailnet address
# and nothing answers an outsider at all. On, `home` and `accounts` are
# proxied names at Cloudflare that land on a listener nginx knows is the
# outside; every other host stays tailnet-only however a name resolves,
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
  # demo in (the gate says who: box/verify/src/lib.rs, demo_door and verify)
  demoParents = map (h: "${h}.${base}") cfg.demoDoors;
  demoNames = map (h: "demo-${h}.${base}") cfg.demoDoors;
  outside = publicNames ++ demoNames;
in
{
  options.dd.public = {
    enable = lib.mkEnableOption "the front door on the open internet";
    hosts = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      description = "the subdomains that answer a connection from outside the tailnet: the gate's own pages (sign-in, sign-up, the demo), what they need, and the network's control server. Everything else is tailnet-only. Lists from every module add up.";
    };
    bare = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "the bare name answers outsiders too, through the tunnel: what people type to find the site. Its server block must offer nothing a stranger may not have (roles/gateway.nix: a redirect to home and the app download).";
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
    tokenFile = lib.mkOption {
      type = lib.types.path;
      description = "file holding CF_DNS_API_TOKEN: dns edit and zone read on the fleet's zone, tunnel edit on the account";
    };
  };

  config = {
    # the tailnet's own view of the names. Tailscale's split DNS sends <base>
    # to this box's tailnet address (one setting there), and this answers
    # every name under it with the tailnet address, so a member's browser
    # takes the tunnel to the gate and never knocks on the public side of
    # its own gateway. Nothing else is answered or forwarded; the boxes
    # themselves have the same answer in /etc/hosts (flake.nix).
    services.dnsmasq = {
      enable = true;
      resolveLocalQueries = false;
      settings = {
        interface = "tailscale0";
        except-interface = "lo";
        bind-dynamic = true;
        no-resolv = true;
        no-hosts = true;
        address = [ "/${base}/${config.dd.box.tailnet}" ];
        # authoritative for the name: an AAAA gets "no such record", not a
        # refusal (a resolver that asks both would fail the whole lookup)
        local = [ "/${base}/" ];
      };
    };

    # the names follow the switch: `*` and the bare name always point at
    # this box's tailnet address, the public hosts are proxied names at
    # Cloudflare while the door is open. Converged at boot and at every
    # switch; nothing here changes on its own, so no timer.
    systemd.services.dd-dns = {
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
      environment = {
        ZONE = base;
        TAILNET = config.dd.box.tailnet;
        TUNNEL = cfg.tunnel;
        HOSTS = lib.concatStringsSep " " (cfg.hosts ++ map (h: "demo-${h}") cfg.demoDoors);
        PUBLIC = lib.boolToString cfg.enable;
        BARE = lib.boolToString cfg.bare;
      };
    };

    # the door itself: an outbound tunnel, not an open port. Each public host
    # is handed to nginx's tunnel listener with its own name, so the same
    # server blocks and certificate serve both sides.
    # over tcp: quic to the edge kept dropping through the carrier's nat
    # ("no recent network activity" every minute); http2 holds
    systemd.services."cloudflared-tunnel-${cfg.tunnel}".environment.TUNNEL_TRANSPORT_PROTOCOL =
      lib.mkIf cfg.enable "http2";
    services.cloudflared = lib.mkIf cfg.enable {
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

    # A connection is inside only when it comes from the tailnet and not
    # through the tunnel's listener. The map says, per connection and host,
    # whether the door is shut; every server block checks it first
    # (commonHttpConfig applies to all), so an outside connection to a
    # tailnet-only name gets a closed connection, not a page, and nothing
    # behind the gate is reachable however the name resolves.
    services.nginx.appendHttpConfig = lib.mkIf cfg.enable ''
        # per address: a ceremony a second, ten in hand
        limit_req_zone $binary_remote_addr zone=dd_ceremony:1m rate=1r/s;
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
          # code, a device joining the network. A person clicks these a few
          # times; a script gets told to wait. The rest of /_dd/ stays
          # unlimited: every endpoint there wants a credential first
          locations."~ ^/_dd/(join|login|enrol|redeem|network)/" = {
            proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}";
            extraConfig = ''
              limit_req zone=dd_ceremony burst=10 nodelay;
              limit_req_status 429;
              proxy_set_header X-Original-URI $request_uri;
            '';
          };
        }
      )
    );
  };
}
