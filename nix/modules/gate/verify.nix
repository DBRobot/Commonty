{
  config,
  options,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.dd.verify;
  full = cfg.role == "full";
  base = config.dd.domain;
  port = config.dd.verify.port;
  user = "dd-verify";
  # a tile's service by its host: https://git.<domain>/ is "git"
  tileHost =
    t: lib.removeSuffix ".${config.dd.domain}" (lib.head (lib.splitString "/" (lib.removePrefix "https://" t.url)));
  demoDoors = if options.dd ? public then config.dd.public.demoDoors else [ ];
in
{
  imports = [
    ./strip-cookie.nix
    ./headers.nix
  ];
  # A box that runs services verifies for them and serves its users' browser
  # login. A box that runs nothing else can still hold the directory: the
  # signed entries are self-authenticating, so a copy on a stranger's box is
  # worth exactly as much as the one here and costs that box no secret.
  options.dd.verify.port = lib.mkOption {
    type = lib.types.port;
    default = 4181;
    description = "where the verifier listens on the box";
  };
  options.dd.verify.hosts = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [
      "files"
      "llm"
      "metrics"
      "git"
      "home"
      "photos"
      "games"
    ];
    description = "the subdomains the gate stands in front of; every one gets the sign-in redirect and the auth subrequest";
  };
  options.dd.verify.enable = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = "Run the verifier. Every box does; a VM test of something the gate plays no part in turns it off, so a change to the gate does not rerun it.";
  };
  options.dd.verify.role = lib.mkOption {

    type = lib.types.enum [
      "full"
      "directory"
    ];
    default = "full";
    description = "full: auth_request for the services on this box plus the directory; directory: the directory alone, on the tailnet.";
  };
  # The other boxes' directories. Pulled every five minutes through the same
  # accept rule a client's update gets; a new name from the network is taken
  # only once every peer has been pulled and none knows it under another
  # root. A box with no peers is on its own and takes new names at once.
  options.dd.verify.syncSeconds = lib.mkOption {
    type = lib.types.int;
    default = 300;
    description = "How often a box pulls its peers' directories. Tests set it low.";
  };
  options.dd.verify.peers = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [ ];
    description = "Directory urls of the other boxes, e.g. https://files.example/_dd/directory.";
  };
  # Every box in the fleet. Settings' Network tab and the Git page show each one's
  # facts as thanos on this box gathered them; no box is asked by another
  # (prometheus answers only on its own box).
  options.dd.verify.fleet = lib.mkOption {
    type = lib.types.attrsOf lib.types.str;
    default = { };
    description = "Box name -> the address its prometheus answers on, e.g. node1 = \"100.95.31.105\".";
  };
  # the front door: one tile per service this box offers, declared by the
  # roles that run them, shown at home.<domain> to whoever is signed in
  options.dd.home.services = lib.mkOption {
    type = lib.types.listOf (
      lib.types.submodule {
        options = {
          name = lib.mkOption { type = lib.types.str; };
          url = lib.mkOption { type = lib.types.str; };
          icon = lib.mkOption {
            type = lib.types.str;
            default = "";
            description = "photos, videos, files, chat, code or metrics; anything else draws a plain mark";
          };
          color = lib.mkOption { type = lib.types.str; };
          blurb = lib.mkOption {
            type = lib.types.str;
            default = "";
            description = "one line under the name on the home page: what it is for";
          };
          rank = lib.mkOption {
            type = lib.types.int;
            default = 50;
            description = "tiles are shown in rank order";
          };
          menuOnly = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = "in the bar's menu, not a tile on the home page";
          };
          demoUrl = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = "Where the demo goes instead, when a member's door and the demo's are not the same one. Null: the same url.";
          };
          demo = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = "What the demo account may do on this tile's host, enforced at the gate: `full` (the service's own permissions are the limit), `read` (no writing method), `rate:N` or `rate:N/M` (reads free, N other requests an hour for each demo - every click on the demo is one - and M for all demos together). Null: nothing; the tile is greyed for it. The demo exists when any tile grants something.";
          };
        };
      }
    );
    default = [ ];
  };

  options.dd.members = lib.mkOption {
    type = lib.types.attrsOf (lib.types.listOf lib.types.str);
    description = "fleet/members.json: `members`, ids let in, and `revoked`, ids shut out (dd member add|remove). Comes with the signed release; a box cannot add to it.";
    default = {
      members = [ ];
      revoked = [ ];
    };
  };
  options.dd.verify.network = lib.mkOption {
    type = lib.types.nullOr (
      lib.types.submodule {
        options = {
          api = lib.mkOption {
            type = lib.types.str;
            description = "headscale's api on this box";
          };
          url = lib.mkOption {
            type = lib.types.str;
            description = "the control server a device is told to join";
          };
          keyFile = lib.mkOption {
            type = lib.types.str;
            description = "file holding headscale's api key for the gate";
          };
        };
      }
    );
    default = null;
    description = "the network's door: with this, an admitted device gets a join key for the fleet's own network (POST /_dd/network/join)";
  };
  # The demo's library: an id and its key, both plain. This is not a
  # secret and must not be dressed as one. A passkey lives in one browser
  # on one device and the demo is one account every visitor shares, so the
  # demo cannot hold a key of its own; the box hands this one to the
  # demo's page, exactly as it already hands over the demo's photos
  # password. Nothing private is in that library, and anyone at all may
  # sign in as the demo and be given the same key.
  # The app the downloads page offers: whatever a manifest signed with the
  # release key names, published beside the box releases. Which tag is
  # current is the manifest's to say, not this box's configuration - so a
  # new app is `dd release app`, not a release of the boxes.
  options.dd.verify.appManifest = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default = null;
    description = "url of the signed app manifest (releases branch, app.json); null: no downloads page";
  };
  # Posters for Movies & TV: a member's device asks TMDB for a title's art
  # with this key and keeps it, sealed, in the library. The key goes to
  # signed-in pages (/_dd/me); the titles never come near the box.
  options.dd.verify.tmdbKeyFile = lib.mkOption {
    type = lib.types.nullOr lib.types.path;
    default = null;
    description = "file holding the fleet's TMDB api key or read token; null: stills and title cards, no posters";
  };
  options.dd.verify.demoLibrary = lib.mkOption {
    type = lib.types.nullOr (
      lib.types.submodule {
        options = {
          id = lib.mkOption { type = lib.types.str; };
          key = lib.mkOption { type = lib.types.str; };
        };
      }
    );
    default = null;
    description = "the library the demo account reads; not a secret, see the comment above";
  };
  options.dd.verify.library = lib.mkOption {
    type = lib.types.nullOr (
      lib.types.submodule {
        options = {
          s3 = lib.mkOption {
            type = lib.types.str;
            description = "the s3 endpoint devices reach with the urls the gate signs";
          };
          bucket = lib.mkOption {
            type = lib.types.str;
            default = "libraries";
          };
          keyFile = lib.mkOption {
            type = lib.types.path;
            description = "env file with LIBRARY_ID and LIBRARY_SECRET";
          };
        };
      }
    );
    default = null;
    description = "the encrypted libraries' gate (modules/library/libraries.nix); null: this box holds none";
  };
  options.dd.verify.photos = lib.mkOption {
    type = lib.types.nullOr (
      lib.types.submodule {
        options = {
          api = lib.mkOption {
            type = lib.types.str;
            description = "museum's address, e.g. https://api.<domain>";
          };
          emailSuffix = lib.mkOption {
            type = lib.types.str;
            description = "the address suffix museum takes our verification code for; a person's ente address is <name><suffix>";
          };
          codeFile = lib.mkOption {
            type = lib.types.str;
            description = "file holding that code, readable by the verifier";
          };
          demoPasswordFile = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = "file holding the demo account's ente password; null: no photos in the demo";
          };
        };
      }
    );
    default = null;
    description = "Photos opened by passkey: the ente account made and opened in the browser (pages::photos). Set by the photos role.";
  };
  options.dd.verify.releasePublicKey = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    description = "The release key, base64: what signs an invite (dd invite). Every box holds invites; the full one checks grants against this.";
    default = null;
  };

  config = lib.mkIf cfg.enable {
    users.users.${user} = {
      isSystemUser = true;
      group = user;
    };
    users.groups.${user} = { };

    # The verifier answers nginx's auth_request for every protected service on
    # this box. A bearer biscuit is checked against the devices in the user's
    # own signed entry; a cookie against the passkeys enrolled here. It holds
    # no key that can sign, so there is nothing in it worth stealing, and it
    # asks no identity server because there is none.
    systemd.services.dd-verify = {
      description = "Verify user-signed tokens and this box's passkey sessions";
      after = [
        "network-online.target"
      ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      environment = {
        VERIFY_ROLE = cfg.role;
        # full sits behind nginx on localhost. directory has no nginx and no
        # public name yet, so it listens on every interface and the firewall
        # lets only the tailnet in: 4181 is not in allowedTCPPorts.
        VERIFY_BIND = if full then "127.0.0.1:${toString port}" else "0.0.0.0:${toString port}";
        VERIFY_DIR = "/var/lib/dd-verify/keys";
        VERIFY_PEERS = lib.concatStringsSep "," cfg.peers;
        VERIFY_FLEET = builtins.toJSON cfg.fleet;
        # every box's facts, from thanos on this box (box/verify/src/fleet.rs)
        VERIFY_THANOS = lib.optionalString (
          config.services.thanos.query.enable or false
        ) "http://${config.services.thanos.query.http-address}";
        VERIFY_SYNC_SECS = toString cfg.syncSeconds;
      }
      // lib.optionalAttrs (full && cfg.appManifest != null) {
        VERIFY_APP_MANIFEST = cfg.appManifest;
      }
      // lib.optionalAttrs (full && cfg.tmdbKeyFile != null) {
        VERIFY_TMDB_KEY_FILE = cfg.tmdbKeyFile;
      }
      // lib.optionalAttrs (full && cfg.demoLibrary != null) {
        VERIFY_DEMO_LIBRARY_ID = cfg.demoLibrary.id;
        VERIFY_DEMO_LIBRARY_KEY = cfg.demoLibrary.key;
      }
      // lib.optionalAttrs (full && cfg.library != null) {
        VERIFY_LIBRARY_S3 = cfg.library.s3;
        VERIFY_LIBRARY_BUCKET = cfg.library.bucket;
      }
      // lib.optionalAttrs (full && cfg.network != null) {
        VERIFY_HEADSCALE_API = cfg.network.api;
        VERIFY_HEADSCALE_URL = cfg.network.url;
        VERIFY_HEADSCALE_KEY_FILE = cfg.network.keyFile;
      }
      // lib.optionalAttrs (full && cfg.photos != null) {
        VERIFY_PHOTOS_API = cfg.photos.api;
        VERIFY_PHOTOS_SUFFIX = cfg.photos.emailSuffix;
        VERIFY_PHOTOS_CODE_FILE = cfg.photos.codeFile;
      }
      // lib.optionalAttrs (full && cfg.photos != null && cfg.photos.demoPasswordFile != null) {
        VERIFY_PHOTOS_DEMO_FILE = cfg.photos.demoPasswordFile;
      }
      // lib.optionalAttrs (cfg.releasePublicKey != null) {
        VERIFY_RELEASE_PUB = cfg.releasePublicKey;
      }
      // lib.optionalAttrs full {
        # the browser login: passkeys scoped to the whole domain, so one login
        # covers every service on this box and the session cookie rides along
        VERIFY_DOMAIN = base;
        # the hosts whose pages may ask for a passkey here (box/verify own_hosts)
        VERIFY_HOSTS = lib.concatStringsSep "," cfg.hosts;
        VERIFY_MEMBERS = builtins.toJSON config.dd.members;
        # our Rust for the browser, next to the pages that use it
        VERIFY_WEB_DIR = "${self.packages.${pkgs.stdenv.hostPlatform.system}.web}";
        # the pages, scripts, styles and icons, apart from the gate program:
        # a page edit is a new directory here, not a new gate
        VERIFY_PAGES_DIR = "${self.packages.${pkgs.stdenv.hostPlatform.system}.pages}";
        VERIFY_HOME = builtins.toJSON (
          map (t: {
            inherit (t)
              name
              url
              icon
              color
              blurb
              demo
              menuOnly
              ;
            # the demo takes the service's demo door where it has one: the
            # same site under demo-<service>, which outsiders may reach
            demoUrl =
              if t.demoUrl != null then
                t.demoUrl
              else if t.demo != null && builtins.elem (tileHost t) demoDoors then
                "https://demo-" + lib.removePrefix "https://" t.url
              else
                null;
          }) (lib.sort (a: b: a.rank < b.rank) config.dd.home.services)
        );
      };
      serviceConfig = {
        Type = "simple";
        User = user;
        Group = user;
        # the gate's bucket key, read by systemd before the drop to `user`
        EnvironmentFile = lib.optional (full && cfg.library != null) cfg.library.keyFile;
        StateDirectory = "dd-verify";
        StateDirectoryMode = "0700";
        ExecStart = "${self.packages.${pkgs.stdenv.hostPlatform.system}.verify}/bin/verify";
        Restart = "on-failure";
        RestartSec = 5;
        NoNewPrivileges = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
        ];
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" ];
      };
    };

    # The session cookie says who you are to the *gate*. It is HttpOnly and
    # Secure, so no page can read it, and it is set for the whole domain so
    # that one sign-in covers every service here. That last part is what put
    # it in every request nginx then forwarded to a backend - grafana,
    # the games manager - none of which need it and any of which,
    # compromised, could have replayed it as that member anywhere.
    #
    # It comes out on the way in, and only it: a backend's own cookies are
    # untouched, and the browser keeps sending it to the gate, so nothing
    # about signing in changes.
    # defined wherever nginx runs, not only on a full gate: a location that
    # names a variable nginx does not know stops nginx starting at all, and
    # nginx is the whole box's front door
    services.nginx.appendHttpConfig = lib.mkIf config.services.nginx.enable ''
      map $http_cookie $dd_cookie_stripped {
        default $http_cookie;
        "~^(?<dd_a>.*?)dd_session=[^;]*;?[ ]?(?<dd_b>.*)$" "$dd_a$dd_b";
      }
      # Flood limits, per credential: a session cookie, or a device's
      # bearer token. Keyed before the gate has said who it is (limits run
      # ahead of auth_request), so by what the request carries; one without
      # either is counted by its address at the public door instead. Sized
      # for a script hammering, never for a person clicking: a page that
      # opens at once fires a few dozen requests, a film a segment a second.
      map "$cookie_dd_session$http_authorization" $dd_credential {
        default "$cookie_dd_session$http_authorization";
      }
      limit_req_zone $dd_credential zone=dd_user:10m rate=30r/s;
      # and by address: the public door's lid on anything at all
      limit_req_zone $binary_remote_addr zone=dd_visitor:10m rate=50r/s;
      # a model's answer is seconds of the box's whole cpu: one message at a
      # time is all a person sends
      limit_req_zone $dd_credential zone=dd_model:1m rate=1r/s;
    '';

    # The verifier's browser side on every vhost that has one: the passkey
    # login and enrolment pages and the directory. /_dd/verify is the auth_request target and internal to nginx. A
    # 401 from auth_request lands a browser on the login page and back where
    # it was.
    services.nginx.virtualHosts = lib.mkIf full (
      lib.mkMerge [
        (builtins.listToAttrs (
          map (h: {
            name = "${h}.${base}";
            value.locations = {
              # A flood from one credential is told to wait; a request with
              # none is counted at the public door (gate/public.nix)
              "/_dd/" = {
                proxyPass = "http://127.0.0.1:${toString port}/_dd/";
                extraConfig = ''
                  proxy_set_header X-Original-URI $request_uri;
                  # a location's limits replace the server's, so both here
                  limit_req zone=dd_user burst=300 nodelay;
                  limit_req zone=dd_visitor burst=500 nodelay;
                  limit_req_status 429;
                '';
              };
              "@login".extraConfig = ''
                return 302 /_dd/login?rd=$request_uri;
              '';
              # one box passing a member's Wi-Fi change to another goes box
              # to box on the fleet's network; nothing comes here for it
              "= /_dd/house/wifi/relay".extraConfig = ''
                return 404;
              '';
              # signed in, not a member: the home page says so
              "@waiting".extraConfig = ''
                return 302 https://home.${base}/;
              '';
              # the subrequest every protected location makes
              "= /_dd/verify" = {
                proxyPass = "http://127.0.0.1:${toString port}/verify";
                extraConfig = ''
                  internal;
                  proxy_pass_request_body off;
                  proxy_set_header Content-Length "";
                  proxy_set_header X-Original-URI $request_uri;
                  # the demo's leash is held in the verifier: it needs the
                  # method and, via Host, where the request was going
                  proxy_set_header X-Original-Method $request_method;
                  proxy_set_header X-Original-Host $host;
                  # The subrequest inherits nothing from the location that
                  # made it, so it ran with nginx's default 1m body limit.
                  # When a large PUT was still arriving as the auth phase ran,
                  # discarding it tripped that limit: "auth request unexpected
                  # status: 413", surfaced as a 500. The limit belongs on the
                  # dav location; here it must not exist.
                  client_max_body_size 0;
                '';
              };
            };
          }) cfg.hosts
        ))
        # the front door itself: home.<domain> is the verifier's page and
        # nothing else. The bare domain cannot carry it (the certificate is
        # the wildcard alone), so the gateway sends it here.
        {
          "home.${base}" = {
            useACMEHost = base;
            forceSSL = true;
            locations."= /" = {
              proxyPass = "http://127.0.0.1:${toString port}/_dd/home";
              extraConfig = "proxy_set_header X-Original-URI $request_uri;";
            };
            # where a QR code from Settings > Devices goes, short to type
            locations."= /add" = {
              proxyPass = "http://127.0.0.1:${toString port}/_dd/add";
              extraConfig = "proxy_set_header X-Original-URI $request_uri;";
            };
          };
        }
      ]
    );
  };
}
