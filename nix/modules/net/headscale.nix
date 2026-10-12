# The fleet's own network control server. Every member's devices and every
# box are nodes of one Headscale tailnet: the app joins it with a key the
# gate hands an admitted device, a box joins it with the boxes' key made
# here. Relays and NAT traversal come from Tailscale's public DERP servers
# until the fleet has a public box of its own, so the control server is
# plain https and rides the front door like the pages do.
#
# Names on this network: Headscale's DNS answers every host nginx serves
# here with this box's address on it (extra records, written once this box
# has joined), so a device on the network reaches the gate directly and
# never through Cloudflare.
{
  config,
  options,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.headscale;
  base = config.dd.domain;
  host = "headscale.${base}";
  port = 8085;
  state = "/var/lib/headscale";
  # who may reach what
  adminPorts = [
    "22"
    "53"
    "80"
    "443"
    "3900"
  ];
  policy = pkgs.writeText "headscale-policy.json" (
    builtins.toJSON {
      tagOwners."tag:box" = [ "boxes@" ];
      # a household's home controller (docs/dev/home-controller.md): a
      # device, joined by the gate in the boxes user so the reaper leaves it
      tagOwners."tag:controller" = [ "boxes@" ];
      # A person's devices reach a box's front door and its game servers.
      # A box's owner's devices reach that box, and only that box, for what
      # an admin needs: ssh, its dns, the cache push. Boxes reach each other
      # on the channels still between them (roles/core.nix lists them; each
      # box's firewall narrows them again to its peers' addresses). No rule
      # has a person's device as its destination, so devices do not see one
      # another, except a home controller's door, on 443 alone: any member
      # may reach any controller there, and the door on the Pi turns away
      # everyone outside its household. The rule names no house.
      acls = [
        {
          action = "accept";
          src = [ "autogroup:member" ];
          dst = [ "tag:controller:443" ];
        }
        # and a controller reaches the boxes' front doors, for its releases,
        # the read-only cache and the directory: what any member's device may
        {
          action = "accept";
          src = [ "tag:controller" ];
          dst = [ "tag:box:443" ];
        }
        {
          action = "accept";
          src = [ "autogroup:member" ];
          dst = [ "tag:box:443" ] ++ map (p: "tag:box:${p}") cfg.gamePorts;
        }
        {
          action = "accept";
          src = [ "tag:box" ];
          dst = map (p: "tag:box:${p}") boxPorts;
        }
      ]
      ++ lib.mapAttrsToList (addr: owner: {
        action = "accept";
        src = [ "${owner}@" ];
        dst = map (p: "${addr}:${p}") adminPorts;
      }) cfg.owners;
    }
  );
  boxPorts = [
    "443"
    "3900"
    "3901"
    "4181"
  ];
  # every name nginx serves on this box, answered on the network with this
  # box's network address (written by the join keeper, dd.net.afterJoin)
  names = lib.filter (n: lib.hasSuffix ".${base}" n || n == base) (
    builtins.attrNames config.services.nginx.virtualHosts
  );
in
{
  options.dd.headscale = {
    enable = lib.mkEnableOption "the fleet's network control server on this box";
    # what the verifier reads to mint join keys, and what a box on this
    # machine joins with; both made once by the seed
    # in the verifier's own state dir: headscale's is its own alone
    apiKeyFile = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/dd-verify/headscale-api.key";
      readOnly = true;
    };
    gamePorts = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      # alone, this box's own (dd.games.reach, the one list of them)
      default = lib.optionals (config.dd ? games) config.dd.games.reach;
      description = "the ports a member's device may reach on a box for games: every games box's range, and every port a game insists on (the flake gives the fleet's; alone, this box's own)";
    };
    owners = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      description = "each box's address on the network -> its owner's name: whose devices administer it (the flake gives every box's)";
    };
    boxes = lib.mkOption {
      type = (lib.types.attrsOf lib.types.attrs);
      default = { };
      description = "the boxes that may ask the gate for a key of their own, and what they sign and are sealed to (dd.verify.network.boxes)";
    };
    url = lib.mkOption {
      type = lib.types.str;
      default = "https://${host}";
      description = "what a device is told to join; plain http on the box's own port in a test";
    };
    behindNginx = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "serve the control server on its public name through nginx (off in a test with no certificate)";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      (lib.optionalAttrs (options.dd ? backup) { dd.backup.paths = [ "${state}/snapshot" ]; })
      {
        services.headscale = {
          enable = true;
          address = "127.0.0.1";
          inherit port;
          settings = {
            server_url = cfg.url;
            # Tailscale's public relays, until the fleet has a public box
            derp = {
              urls = [ "https://controlplane.tailscale.com/derpmap/default" ];
              server.enabled = false;
            };
            dns = {
              magic_dns = true;
              base_domain = "net.${base}";
              # Quad9's unfiltered pair, nothing blocked: the same answers as
              # before, from a resolver that is not the front door's company
              nameservers.global = [
                "9.9.9.10"
                "149.112.112.10"
                "2620:fe::10"
                "2620:fe::fe:10"
              ];
              extra_records_path = "${state}/extra-records.json";
            };
            policy = {
              mode = "file";
              path = "${policy}";
            };
            # nodes stay until removed; a laptop off for a month is still yours
            ephemeral_node_inactivity_timeout = "30m";
          };
        };

        # Who is on the network lives in its database: lost, every device of
        # every member joins again. A copy made with sqlite's own backup (the
        # live file is mid-write), with the server's key, before each backup.
        systemd.services.headscale-snapshot = {
          description = "Copy the network's database for the backup";
          before = [ "restic-backups-dd.service" ];
          wantedBy = [ "restic-backups-dd.service" ];
          serviceConfig = {
            Type = "oneshot";
            User = "headscale";
            Group = "headscale";
          };
          script = ''
            umask 077
            mkdir -p ${state}/snapshot
            ${pkgs.sqlite}/bin/sqlite3 ${state}/db.sqlite ".backup ${state}/snapshot/db.sqlite.new"
            mv ${state}/snapshot/db.sqlite.new ${state}/snapshot/db.sqlite
            cp ${state}/noise_private.key ${state}/snapshot/noise_private.key
          '';
        };

        # the names file exists before the first start (empty) and is filled
        # once this box has joined
        systemd.tmpfiles.rules = [ "f ${state}/extra-records.json 0644 headscale headscale - []" ];

        # made once: the boxes' user and the verifier's api key. A file, not a
        # secret in sops, because this box makes it for itself. A box's key to
        # join is never kept: each is made when it is wanted (dd.net.getKey).
        systemd.services.headscale-seed = {
          description = "Seed the network: the boxes' user, the verifier's api key";
          after = [ "headscale.service" ];
          requires = [ "headscale.service" ];
          wantedBy = [ "multi-user.target" ];
          path = [
            config.services.headscale.package
            pkgs.jq
            pkgs.coreutils
          ];
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
          };
          script = ''
            set -eu
            for _ in $(seq 1 60); do headscale users list >/dev/null 2>&1 && break; sleep 1; done
            # an empty list is json null here
            who='(. // []) | .[] | select(.name == "boxes") | .id'
            id=$(headscale users list -o json | jq -r "$who")
            if [ -z "$id" ]; then
              headscale users create boxes >/dev/null
              id=$(headscale users list -o json | jq -r "$who")
            fi
            if [ ! -s ${cfg.apiKeyFile} ]; then
              mkdir -p "$(dirname ${cfg.apiKeyFile})"
              headscale apikeys create --expiration 3650d > ${cfg.apiKeyFile}.tmp
              chown root:dd-verify ${cfg.apiKeyFile}.tmp
              chmod 0440 ${cfg.apiKeyFile}.tmp
              mv ${cfg.apiKeyFile}.tmp ${cfg.apiKeyFile}
            fi
          '';
        };

        # the names, once this box is on its own network and has an address
        # there (run by the join keeper). Headscale watches the file.
        dd.net.afterJoin = ''
          ip=$(tailscale --socket ${config.dd.net.socket} ip -4)
          jq -n --arg ip "$ip" '[${
            lib.concatMapStringsSep ", " (n: ''{"name": "${n}", "type": "A", "value": $ip}'') names
          }]' > ${state}/extra-records.json.tmp
          chown headscale:headscale ${state}/extra-records.json.tmp
          mv ${state}/extra-records.json.tmp ${state}/extra-records.json
          echo "the fleet's names answer with $ip on the network"
        '';

        # the gate hands admitted devices their join keys through headscale's api
        dd.verify.network = {
          api = "http://127.0.0.1:${toString port}";
          url = cfg.url;
          keyFile = cfg.apiKeyFile;
          inherit (cfg) boxes;
        };
        # the verifier starts after the key exists
        systemd.services.dd-verify.after = [ "headscale-seed.service" ];
        systemd.services.dd-verify.wants = [ "headscale-seed.service" ];

        # this box joins its own network with the key it just made: a member's
        # device reaching the fleet arrives here, so this end has to be on it
        dd.net.enable = true;
        # its own key, made here: one use, ten minutes
        dd.net.getKey = ''
          id=$(${config.services.headscale.package}/bin/headscale users list -o json \
            | jq -r '(. // []) | .[] | select(.name == "boxes") | .id')
          ${config.services.headscale.package}/bin/headscale preauthkeys create \
            --user "$id" --expiration 10m --tags tag:box > "$key"
        '';
        dd.net.controlUrl = cfg.url;
        systemd.services.commonty-net-up.after = [ "headscale-seed.service" ];
        systemd.services.commonty-net-up.wants = [ "headscale-seed.service" ];

        # Reached from outside through the front door (the gateway role lists
        # the host as public) and from the boxes by the pinned name. The
        # control protocol is an http upgrade that is not websocket, and the
        # door's proxy strips it: the app carries it over a real websocket
        # (its bridge, app/net/bridge.go), which the server takes as well; a
        # box's stock tailscaled reaches this box directly. Both upgrades pass
        # nginx the same way.
        services.nginx.virtualHosts.${host} = lib.mkIf cfg.behindNginx {
          useACMEHost = base;
          forceSSL = true;
          locations."/" = {
            proxyPass = "http://127.0.0.1:${toString port}";
            proxyWebsockets = true;
            extraConfig = ''
              proxy_buffering off;
              proxy_read_timeout 1h;
            '';
          };
          # the admin api and its docs are for the verifier, which asks on
          # 127.0.0.1; through the door they are one stolen key from the
          # whole network
          locations."/api/".return = "404";
          locations."/swagger".return = "404";
        };
      }
    ]
  );
}
