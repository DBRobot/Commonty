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
  # a gate with a domain and sessions: the full one, or a box's own pages
  serving = full || cfg.role == "box";
  base = config.dd.domain;
  port = config.dd.verify.port;
  user = "dd-verify";
  # a tile's service by its host: https://git.<domain>/ is "git"
  tileHost =
    t:
    lib.removeSuffix ".${config.dd.domain}" (
      lib.head (lib.splitString "/" (lib.removePrefix "https://" t.url))
    );
  # the directory is the control plane's agreed log wherever the control
  # plane runs (servers/gate/src/directory.rs, Agreed)
  agreed = options.dd ? control && config.dd.control.enable;
  demoDoors = if options.dd ? public then config.dd.public.demoDoors else [ ];
in
{
  imports = [
    ./strip-cookie.nix
    ./headers.nix
    # the fleet's own hosts, on every box; a module adds its own beside them
    {
      dd.verify.hosts = [
        "files"
        "llm"
        "metrics"
        "git"
        "home"
        "photos"
        "games"
      ];
    }
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
    description = "the subdomains the gate stands in front of; every one gets the sign-in redirect and the auth subrequest. Lists from every module add up.";
  };
  options.dd.verify.enable = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = "Run the verifier. Every box does; a VM test of something the gate plays no part in turns it off, so a change to the gate does not rerun it.";
  };
  options.dd.verify.admin = lib.mkOption {
    type = lib.types.str;
    default = "";
    description = "the fleet's admin (fleet/policy.json owner): sees every box's metrics. A box's own owner sees that box's";
  };
  options.dd.verify.role = lib.mkOption {

    type = lib.types.enum [
      "full"
      "box"
      "directory"
    ];
    default = "full";
    description = "full: auth_request for the services on this box plus the directory; box: this box's own pages (its metrics) for its owner and the admin, and the directory; directory: the directory alone, on the tailnet.";
  };
  # The mail Worker's directory read key (fleet/mail.json reader): single
  # entries are shown to it; to nobody else without a credential. Empty: a
  # single entry is anyone's, until the Worker has its key.
  options.dd.verify.directoryReaders = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [ ];
    description = "Base64 ed25519 public keys that may read any one directory entry, never the list (the mail Worker's).";
  };
  # What a token's audience may name here, besides the domain's own hosts:
  # the box's address on the fleet's network, where `dd` asks a directory
  # box. A Host header adds nothing to it (servers/gate directory.rs here).
  options.dd.verify.names = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = lib.optional (config.dd.box.net or "" != "") config.dd.box.net;
    defaultText = lib.literalExpression "[ config.dd.box.net ]";
    description = "Extra names this box answers to, for device tokens' audience (VERIFY_NAMES).";
  };
  options.dd.verify.leaveWindowSeconds = lib.mkOption {
    type = lib.types.nullOr lib.types.int;
    default = null;
    description = "how long someone's things are kept after they leave; null: 30 days. A test shortens it.";
  };
  options.dd.verify.housePeers = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [ ];
    description = "the other boxes in this house (same site), as directory urls: a Wi-Fi change is relayed to them";
  };
  # Every box in the fleet. Settings' Network tab and the Git page show each one's
  # facts as thanos on this box gathered them; no box is asked by another
  # (prometheus answers only on its own box).
  options.dd.verify.boxes = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule {
        options.owner = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
        };
        options.metrics = lib.mkOption {
          type = lib.types.str;
          default = "";
        };
      }
    );
    default = { };
    description = "Every box: whose it is (box.json owner) and its own metrics name, if it has one. The Metrics page shows a person the boxes they own, and the admin every box.";
  };
  options.dd.verify.fleet = lib.mkOption {
    type = lib.types.attrsOf lib.types.str;
    default = { };
    description = "Box name -> the address its prometheus answers on, e.g. node1 = \"100.64.0.2\".";
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
    type = lib.types.attrsOf lib.types.anything;
    description = "fleet/members.json: `members`, ids let in, and `revoked`, ids shut out (dd member add|remove); `removed`, when, and `names`, each id's name tag, beside them. Comes with the signed release; a box cannot add to it.";
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
          boxes = lib.mkOption {
            type = lib.types.attrsOf (
              lib.types.submodule {
                options = {
                  key = lib.mkOption {
                    type = lib.types.str;
                    description = "the P-256 key in the box's TPM it signs with (base64 SPKI, boxes.json unlockKey)";
                  };
                  recipient = lib.mkOption {
                    type = lib.types.str;
                    description = "the age recipient of the box's TPM secrets key (its signed hardware.json, secretsKey)";
                  };
                };
              }
            );
            default = { };
            description = "boxes that may ask for a key of their own (POST /_dd/network/box), answered sealed to them";
          };
        };
      }
    );
    default = null;
    description = "the network's door: with this, an admitted device gets a join key for the fleet's own network (POST /_dd/network/join)";
  };
  # The app the downloads page offers: whatever a manifest signed with the
  # release key names, published beside the box releases. Which tag is
  # current is the manifest's to say, not this box's configuration - so a
  # new app is `dd release app`, not a release of the boxes.
  options.dd.verify.appManifest = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default = null;
    description = "url of the signed app manifest (releases branch, app.json); null: no downloads page";
  };
  # The member list a client holds other members' entries to: what `dd
  # release publish` signed beside the box releases, passed on as it is.
  # The client checks the signature; the box only serves it, so a device
  # that reaches nothing but the front door can still get it.
  options.dd.verify.memberList = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default = "https://git.${config.dd.domain}/${config.dd.repo}/raw/branch/releases/members.json";
    defaultText = lib.literalExpression ''"https://git.''${config.dd.domain}/''${config.dd.repo}/raw/branch/releases/members.json"'';
    description = "url of the signed member list (releases branch, members.json), served at /_dd/members; null: not served";
  };
  # The release notes the uptime page and the corner assistant read
  # (/_dd/releases.json): the forge's own list, through its public door as
  # nobody, so only what a stranger may read already.
  options.dd.verify.releases = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default =
      if config.services.forgejo.enable then
        "https://code.${config.dd.domain}/api/v1/repos/${config.dd.repo}/releases?limit=50"
      else
        null;
    defaultText = lib.literalExpression ''"https://code.''${config.dd.domain}/api/v1/repos/''${config.dd.repo}/releases?limit=50" when the forge is here'';
    description = "url of the forge's release list for this fleet's repository; null: no release notes";
  };
  # Whose word counts on whether a service works: the boxes whose roles
  # run it (roles/core.nix, from fleet/boxes.json). Any other box's note
  # about it is ignored (servers/gate/src/status.rs).
  options.dd.verify.serviceBoxes = lib.mkOption {
    type = lib.types.attrsOf (lib.types.listOf lib.types.str);
    default = { };
    description = "uptime service id -> the boxes whose roles run it; empty: no service is shown";
  };
  # Posters for Movies & TV: a member's device asks TMDB for a title's art
  # with this key and keeps it, sealed, in the library. The key goes to
  # signed-in pages (/_dd/me); the titles never come near the box.
  options.dd.verify.tmdbKeyFile = lib.mkOption {
    type = lib.types.nullOr lib.types.path;
    default = null;
    description = "file holding the fleet's TMDB api key or read token; null: stills and title cards, no posters";
  };
  # The demo's library: an id and its key, both plain. This is not a
  # secret and must not be dressed as one. A passkey lives in one browser
  # on one device and the demo is one account every visitor shares, so the
  # demo cannot hold a key of its own; the box hands this one to the
  # demo's page, exactly as it already hands over the demo's photos
  # password. Nothing private is in that library, and anyone at all may
  # sign in as the demo and be given the same key.
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
            description = "the s3 endpoint the gate reaches the bucket at, and the transcoder with the one url it is handed";
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
      ]
      ++ lib.optional agreed "dd-control.service";
      wants = [ "network-online.target" ] ++ lib.optional agreed "dd-control.service";
      wantedBy = [ "multi-user.target" ];
      environment = {
        VERIFY_ROLE = cfg.role;
        # full sits behind nginx on localhost. directory has no nginx and no
        # public name yet, so it listens on every interface and the firewall
        # lets only the tailnet in: 4181 is not in allowedTCPPorts.
        # With the agreed log (the control plane) nobody pulls this box's
        # directory; a box that is not the front door still takes its
        # house's Wi-Fi relay, and the firewall lets in only that house's
        # boxes (roles/core.nix)
        VERIFY_BIND =
          if full || (agreed && cfg.housePeers == [ ]) then
            "127.0.0.1:${toString port}"
          else
            "0.0.0.0:${toString port}";
        VERIFY_DIR = "/var/lib/dd-verify/keys";
        # the house's own boxes, for its Wi-Fi and its page; never the fleet
        VERIFY_HOUSE_PEERS = lib.concatStringsSep "," cfg.housePeers;
        VERIFY_FLEET = builtins.toJSON cfg.fleet;
        # every box's facts, from thanos on this box (servers/gate/src/fleet.rs)
        VERIFY_THANOS = lib.optionalString (config.services.thanos.query.enable or false
        ) "http://${config.services.thanos.query.http-address}";
        VERIFY_DIRECTORY_READERS = lib.concatStringsSep "," cfg.directoryReaders;
        VERIFY_NAMES = lib.concatStringsSep "," cfg.names;
        # who sees this box as a whole (its metrics): its owner, and the admin
        # who sees every box's
        VERIFY_OWNER = lib.optionalString (config.dd.box.owner or null != null) config.dd.box.owner;
        VERIFY_ADMIN = cfg.admin;
      }
      # the directory as the control plane's agreed log
      // lib.optionalAttrs agreed {
        VERIFY_CONTROL = "http://127.0.0.1:4195";
        VERIFY_CONTROL_SUBMIT = "/run/dd-control/submit.sock";
        VERIFY_CONTROL_CONFIG = "/etc/dd-control/config.json";
      }
      // lib.optionalAttrs (cfg.memberList != null) {
        VERIFY_MEMBER_LIST = cfg.memberList;
      }
      // lib.optionalAttrs (full && cfg.releases != null) {
        VERIFY_RELEASES = cfg.releases;
      }
      // lib.optionalAttrs full {
        VERIFY_SERVICE_BOXES = builtins.toJSON cfg.serviceBoxes;
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
        VERIFY_NETWORK_BOXES = "${pkgs.writeText "network-boxes.json" (builtins.toJSON cfg.network.boxes)}";
        # the boxes' recipients are TPM keys: age needs the plugin to seal to one
        VERIFY_AGE = "${pkgs.writeShellScript "age-for-boxes" ''
          PATH=${lib.makeBinPath [ pkgs.age-plugin-tpm ]}:$PATH exec ${pkgs.age}/bin/age "$@"
        ''}";
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
      // lib.optionalAttrs (cfg.leaveWindowSeconds != null) {
        VERIFY_LEAVE_WINDOW_SECS = toString cfg.leaveWindowSeconds;
      }
      # the front door's gate asks the mail Worker to delete the address of
      # whoever left 30 days ago (servers/gate/src/leaving.rs)
      // lib.optionalAttrs (full && options.dd ? public && config.dd.public.enable) {
        VERIFY_MAIL_FORGET = "https://mail.${base}/api/forget";
      }
      // lib.optionalAttrs serving {
        # the browser login: passkeys scoped to the whole domain, so one login
        # covers every service on this box and the session cookie rides along
        VERIFY_DOMAIN = base;
        # where the docs live on the forge, for a member's edit and history links
        VERIFY_REPO = config.dd.repo;
        # the hosts whose pages may ask for a passkey here (servers/gate own_hosts)
        VERIFY_HOSTS = lib.concatStringsSep "," cfg.hosts;
        # sign-in at the id Worker, when it is set up (sso.rs)
        VERIFY_ID = config.dd.idWorker;
        VERIFY_BOXES = builtins.toJSON cfg.boxes;
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
        ]
        ++ lib.optional agreed "AF_UNIX"; # dd-control's submit socket
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
              # a friend link's secret is its path, and the access log keeps
              # paths: these are not logged at all
              "~ ^/_dd/friend/" = {
                proxyPass = "http://127.0.0.1:${toString port}";
                extraConfig = ''
                  access_log off;
                  proxy_set_header X-Original-URI $request_uri;
                  limit_req zone=dd_user burst=300 nodelay;
                  limit_req zone=dd_visitor burst=500 nodelay;
                  limit_req_status 429;
                '';
              };
              # this box's link is for its own processes, not the house's
              # page through here (servers/gate/src/home.rs)
              "= /_dd/house/here".extraConfig = ''
                return 404;
              '';
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
                  # method and, via X-Original-Host (strip-cookie.nix sets it
                  # on every gate location), where the request was going
                  proxy_set_header X-Original-Method $request_method;
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
        # nothing else. The bare domain sends people here (gateway.nix).
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
