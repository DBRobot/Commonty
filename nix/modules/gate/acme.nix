{
  config,
  lib,
  pkgs,
  ...
}:
let
  base = config.dd.domain;
  cfg = config.dd.acme;
  joined = config.dd.box.joined or false;
  # a box that joined on its own key has its store's name: what else its
  # nginx serves is no name the DNS Worker gives it, and one name refused
  # fails a certificate's whole order
  names =
    if joined then
      [ "store.${config.networking.hostName}.${config.dd.joinedDomain}" ]
    else
      import ../../lib/https-names.nix lib config;
  # and a certificate of its own for each name of a service the plan may give
  # it, ordered only while the plan does (box/placed.nix's flag): what the
  # Worker gives it moves with the plan, a certificate's names do not
  needs = import ../../lib/needs.nix {
    inherit lib;
    on = config.dd.failover or false;
  };
  planned = lib.optionals joined (
    lib.concatMap (
      svc:
      lib.optionals (needs ? ${svc} && needs.${svc}.placed && !needs.${svc}.lease) (
        map (h: {
          name = "${h}.${config.dd.joinedDomain}";
          inherit svc;
        }) needs.${svc}.hosts
      )
    ) (config.dd.placed.services or [ ])
  );
  legoEnv = pkgs.writeText "acme-exec.env" ''
    EXEC_PATH=${pkgs.writeShellScript "acme-exec" ''
      PATH=${lib.makeBinPath [ pkgs.socat ]}
      ${builtins.readFile ./acme-exec.sh}
    ''}
    EXEC_PROPAGATION_TIMEOUT=300
    EXEC_POLLING_INTERVAL=5
  '';
in
{
  options.dd.acme = {
    enable = lib.mkEnableOption "a certificate for the names this box's nginx serves, and nginx";
    worker = lib.mkOption {
      type = lib.types.str;
      default = config.dd.dnsWorker;
      description = "the DNS Worker (servers/system/dns) that writes this box's challenges; empty: the wildcard, by the box's own DNS token";
    };
    softKey = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "a test's P-256 key (PEM) in place of the TPM's";
    };
  };

  imports = [ ../box/remote-jobs.nix ];

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        # An order asks Let's Encrypt, and the DNS Worker for the
        # challenge: never inside a switch (box/remote-jobs.nix). nixpkgs
        # has the certificate's own unit want the order, which put it in
        # the switch's transaction whenever the certificate's names
        # changed; the order's timer starts it after the switch instead
        dd.remoteJobs."acme-order-renew-${base}" = { };
        systemd.services."acme-${base}".wants = lib.mkForce [ "acme-setup.service" ];
      }
      (lib.mkIf (cfg.worker != "") {
        # The names this box serves and no others, each proved by a challenge
        # the DNS Worker writes: no box holds a token for the zone. lego asks
        # the root helper over a socket; the helper signs with the TPM.
        security.acme.certs.${base} = {
          domain = lib.head names;
          extraDomainNames = lib.tail names;
          dnsProvider = "exec";
          environmentFile = legoEnv;
        };
        systemd.services."acme-order-renew-${base}" = {
          requires = [ "dd-acme.socket" ];
          after = [ "dd-acme.socket" ];
        };

        systemd.sockets.dd-acme = {
          description = "Challenges for this box's certificate, asked of the DNS Worker";
          wantedBy = [ "sockets.target" ];
          socketConfig = {
            ListenStream = "/run/dd-acme.sock";
            # lego runs as acme; its group is the certificates' group, which
            # nginx is in, so the socket is the user's alone
            SocketUser = "acme";
            SocketMode = "0600";
            Accept = true;
            MaxConnections = 4;
          };
        };
        systemd.services."dd-acme@" = {
          description = "Ask the DNS Worker for one challenge";
          path = [
            pkgs.coreutils
            pkgs.gnugrep
            pkgs.jq
            pkgs.curl
            pkgs.openssl
            pkgs.tpm2-tools
            pkgs.util-linux # flock: one user of the TPM at a time
          ];
          environment.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
          script = (import ../../lib/script.nix { inherit lib; }) ./acme-ask.sh {
            BOX = config.networking.hostName;
            WORKER = cfg.worker;
            NAMES = lib.concatStringsSep "\n" (names ++ map (p: p.name) planned);
            KEYS = "/boot/dd/unlock";
            SOFT_KEY = if cfg.softKey != null then cfg.softKey else "";
          };
          serviceConfig = {
            StandardInput = "socket";
            StandardOutput = "socket";
            StandardError = "journal";
            TimeoutSec = 90;
            PrivateTmp = true;
            ProtectHome = true;
            ProtectSystem = "strict";
            NoNewPrivileges = true;
          };
        };
      })
      (lib.mkIf (cfg.worker != "" && planned != [ ]) {
        security.acme.certs = lib.listToAttrs (
          map (
            p:
            lib.nameValuePair p.name {
              domain = p.name;
              group = "acmecerts";
              dnsProvider = "exec";
              environmentFile = legoEnv;
            }
          ) planned
        );
        services.nginx.virtualHosts = lib.listToAttrs (
          map (p: lib.nameValuePair p.name { useACMEHost = lib.mkForce p.name; }) planned
        );
        dd.remoteJobs = lib.listToAttrs (
          map (p: lib.nameValuePair "acme-order-renew-${p.name}" { }) planned
        );
        systemd.services = lib.mkMerge (
          map (p: {
            "acme-${p.name}".wants = lib.mkForce [ "acme-setup.service" ];
            "acme-order-renew-${p.name}" = {
              requires = [ "dd-acme.socket" ];
              after = [ "dd-acme.socket" ];
              unitConfig.ConditionPathExists = "/run/dd-placed/${p.svc}";
            };
          }) planned
        );
        # the plan gives it the service: its certificate now, not at the next
        # renewal's check
        systemd.paths = lib.listToAttrs (
          map (
            p:
            lib.nameValuePair "acme-order-renew-${p.name}" {
              wantedBy = [ "multi-user.target" ];
              pathConfig.PathChanged = "/run/dd-placed/${p.svc}";
            }
          ) planned
        );
      })
      {
        # One wildcard cert for every subdomain and the bare name, until the acme
        # Worker is set up. DNS-01 needs no inbound ports, which is why nothing
        # here opens 80/443 - tailscale0 is already trusted. The challenge writes
        # a TXT record at Cloudflare with the same token the front door uses.
        security.acme = {
          acceptTerms = true;
          # The fleet's own security@ alias (fleet/policy.json mailAliases), not a
          # personal address: the module passes it to lego as --email at eval
          # time, so it is in the store and in this public repo. Changing it
          # re-registers the acme account (the address is hashed into the account
          # directory); issued certs are unaffected.
          defaults.email = "security@${base}";
          certs.${base} = {
            # a group rather than the nginx user: kanidm once read these too, and
            # the next service that terminates its own tls will again
            group = "acmecerts";
          }
          // lib.optionalAttrs (cfg.worker == "") {
            domain = "*.${base}";
            extraDomainNames = [ base ];
            dnsProvider = "cloudflare";
            environmentFile = config.sops.templates."cloudflare.env".path;
          };
        };

        users.groups.acmecerts = { };
        users.users.nginx.extraGroups = [ "acmecerts" ];

        services.nginx = {
          enable = true;
          recommendedProxySettings = true;
          # the chat page alone is a 5M script; a phone on the tailnet felt it
          recommendedGzipSettings = true;
          # A name nothing here serves (a retired one, or a guess) used to land on
          # whichever vhost nginx listed first. Now it gets no tls handshake at all.
          virtualHosts."_" = {
            default = true;
            rejectSSL = true;
            locations."/".return = "444";
          };
          # The access log keeps the path and never the query string: that is
          # where share links, one-time codes and search terms travel. The
          # referrer, which is another page's whole address, is cut the same way,
          # and a friend link's page to before its secret, which is its path.
          appendHttpConfig = ''
            map $http_referer $dd_referer_path {
              default "";
              "~^(?<dd_ref>[^?#]*?/_dd/friend/)" $dd_ref;
              "~^(?<dd_ref>[^?#]*)" $dd_ref;
            }
            log_format dd_path '$remote_addr - $remote_user [$time_local] '
                               '"$request_method $uri $server_protocol" $status $body_bytes_sent '
                               '"$dd_referer_path" "$http_user_agent"';
            access_log /var/log/nginx/access.log dd_path;
          '';
        };
        # and it is kept 20 days, a day at a time, not the half year logrotate would
        services.logrotate.settings.nginx = {
          frequency = lib.mkForce "daily";
          rotate = lib.mkForce 20;
        };
      }
    ]
  );
}
