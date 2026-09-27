{
  config,
  pkgs,
  lib,
  ddScript,
  ...
}:
let
  base = config.dd.domain;
  museumPort = 8080;
  d = sub: "${sub}.${base}";
  entePorts = map d [
    "api"
    "accounts"
    "albums"
    "cast"
    "photos"
  ];

  # Locker is a fourth ente app - notes, credentials, physical records and
  # documents - sharing the account and museum that photos already uses. The
  # nixos module only knows about accounts/albums/cast/photos, so it is built
  # and served here with the same overrides the module applies to those.
  lockerPkg = pkgs.ente-web.override {
    enteApp = "locker";
    enteMainUrl = "https://${d "photos"}";
    extraBuildEnv = {
      NEXT_PUBLIC_ENTE_ENDPOINT = "https://${d "api"}";
      NEXT_PUBLIC_ENTE_ALBUMS_ENDPOINT = "https://${d "albums"}";
      NEXT_TELEMETRY_DISABLED = "1";
    };
  };
in
{
  options.dd.photos.admin = lib.mkOption {
    type = lib.types.int;
    description = "museum's id for the fleet's owner: the one account that may call museum's admin api. Museum numbers accounts itself, so this is read off after the owner's account is made (dd status shows it).";
  };

  config = {
    # One fixed ente, whatever nixpkgs moves to: the photos page hands ente's
    # web app a session in the shape this version reads, and museum takes
    # our verification code the way this version does. Bumping is a decision
    # here, with the page checked against it, not a side effect of a nixpkgs
    # update.
    nixpkgs.overlays = [
      (final: prev: {
        museum = prev.museum.overrideAttrs (o: {
          version = "1.3.36";
          src = prev.fetchFromGitHub {
            owner = "ente";
            repo = "ente";
            rev = "photos-v1.3.36";
            hash = "sha256-9MWmJ3QUgS7BToTnSZzTi4ywGW1RtwrCO+9yQJkvejM=";
          };
        });
        ente-web = prev.ente-web.overrideAttrs (o: {
          version = "1.3.36";
          src = prev.fetchFromGitHub {
            owner = "ente";
            repo = "ente";
            rev = "photos-v1.3.36";
            hash = "sha256-o75r8LFgG3BT3IIPiD9x6gY3fRDoxJ3ZTBPAYr3hLWI=";
          };
          # every way out of the app that would show ente's own sign-in or
          # sign-up goes to the passkey page instead: the app is reached only
          # through it
          # It also wears the site's look: its theme set to the site's colours,
          # type and corners, and the site's own bar across the top
          patches = (o.patches or [ ]) ++ [
            ./ente-web-passkey.patch
            ./ente-web-commonty.patch
            # the menu: the site's, with a passkey where Ente asks for the
            # password nobody here knows, and nothing that needs it (hiding)
            ./ente-web-commonty-menu.patch
          ];
          # Next names each build at random, and every page carries the
          # name: two builds of the same source never matched, and the
          # boxes vouch for a release by rebuilding it (dd-attest)
          postPatch = (o.postPatch or "") + ''
            substituteInPlace packages/base/next.config.base.js \
              --replace-fail 'output: "export",' "output: \"export\", generateBuildId: async () => \"commonty\","
            # the bar's stylesheet is the site's own file, not a copy of it
            cp ${../../../box/verify/web/bar.css} apps/photos/src/styles/commonty-bar.css
            # The grid drew three rows past the screen, and a picture is only
            # fetched once it is drawn: scrolling reached pictures still on
            # their way. Twenty rows is a few screens of warning.
            substituteInPlace apps/photos/src/components/FileList.tsx \
              --replace-fail "overscanCount={3}" "overscanCount={20}"
          '';
        });
      })
    ];

    # the login route, probed every few minutes; a 500 there is the known
    # panic and a restart is the cure (ente-health.sh)
    systemd.services.ente-health = {
      description = "Restart museum when its login route is broken";
      after = [ "ente.service" ];
      path = [ pkgs.curl ];
      serviceConfig.Type = "oneshot";
      script = ddScript ./ente-health.sh { PORT = toString museumPort; };
    };
    systemd.timers.ente-health = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "5min";
        OnUnitActiveSec = "3min";
      };
    };

    services.ente = {
      web = {
        enable = true;
        domains = {
          accounts = d "accounts";
          albums = d "albums";
          cast = d "cast";
          photos = d "photos";
        };
      };
      api = {
        enable = true;
        nginx.enable = true;
        enableLocalDB = true; # peer auth over a socket, so no db password exists
        domain = d "api";
        settings = {
          s3 = {
            use_path_style_urls = true;
            b2-eu-cen = {
              endpoint = "https://${d "s3"}";
              region = "us-east-1"; # required internally by ente regardless of reality
              bucket = "ente";
              key._secret = config.sops.secrets.garage-key-id.path;
              secret._secret = config.sops.secrets.garage-key-secret.path;
            };
          };
          key = {
            encryption._secret = config.sops.secrets.ente-key-encryption.path;
            hash._secret = config.sops.secrets.ente-key-hash.path;
          };
          jwt.secret._secret = config.sops.secrets.ente-jwt-secret.path;
          # Addresses under users.<domain> are ours: a person's ente account
          # is <name>@users.<domain>, made by the photos page with this code
          # instead of a mail nobody would receive. Museum honours it because
          # the nixos module runs it as ENVIRONMENT=local.
          # who may call museum's admin api (dd photos-demo sets the demo's quota)
          internal.admins = [ config.dd.photos.admin ];
          internal.hardcoded-ott = {
            local-domain-suffix = "@users.${base}";
            local-domain-value._secret = config.sops.secrets.ente-ott.path;
          };
          # gomail does STARTTLS on its own unless SSL is set, so 587/tls rather
          # than 465/ssl. Gmail rewrites From to the authenticated account, so
          # the sender address cannot be one of our own subdomains.
          smtp = {
            host = "smtp.gmail.com";
            port = 587;
            encryption = "tls";
            username = "distributed.datacenter@gmail.com";
            email = "distributed.datacenter@gmail.com";
            sender-name = "Ente";
            password._secret = config.sops.secrets.ente-smtp-password.path;
          };
        };
      };
    };

    services.nginx.virtualHosts =
      lib.genAttrs entePorts (_: {
        useACMEHost = base;
        forceSSL = true;
      })
      // {
        ${d "locker"} = {
          useACMEHost = base;
          forceSSL = true;
          locations."/" = {
            root = lockerPkg;
            tryFiles = "$uri $uri.html /index.html";
          };
        };
      };
  };
}
