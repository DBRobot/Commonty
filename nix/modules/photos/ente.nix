{
  config,
  options,
  pkgs,
  lib,
  ddScript,
  ...
}:
let
  base = config.dd.domain;
  museumPort = 8080;
  d = sub: "${sub}.${base}";
  # with failover on (box/box.nix), museum's keys are this box's own, made
  # here, and its photos are in this box's own store; a box that takes
  # photos over makes its own and carries the accounts over (museum's
  # dd_rekey, servers/photos/museum-own-keys.patch)
  own = config.dd.failover or false;
  keys = "/var/lib/dd-ente-keys";
  secret = name: sops: if own then "${keys}/${name}" else config.sops.secrets.${sops}.path;
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
  # its database is dumped where it lives (storage/postgres-backup.nix)
  imports = [ ../storage/postgres-backup.nix ];

  config = lib.mkMerge [
    (lib.optionalAttrs (options.dd ? store) {
      # browsers upload to the bucket straight (see the role's cors)
      dd.store.buckets.ente.cors = lib.mkIf own true;
      # the store, at the name museum hands browsers: its signatures are for
      # this name, which nginx passes on
      services.nginx.virtualHosts.${d "photos-store"} = lib.mkIf own {
        useACMEHost = base;
        forceSSL = true;
        locations."/" = {
          proxyPass = "http://127.0.0.1:${toString config.dd.store.port}";
          # s3 signs its requests; no cookie of anyone's goes to the store
          ddCookie = "";
          extraConfig = ''
            client_max_body_size 0;
            proxy_request_buffering off;
            proxy_set_header Host $host;
          '';
        };
      };
    })
    {
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
            # sessions kept as sha256 of their token: a copy of the database
            # signs nobody in. Newer museum does this itself (its migrations
            # 134+), from the plain tokens: a bump has to carry these hashes
            # into its token_hash column by hand, not hash them again
            patches = (o.patches or [ ]) ++ [
              ../../../servers/photos/museum-hash-tokens.patch
              # deleted means deleted: a file's objects go at the next run
              # once its trash is emptied, and an account's data when its
              # deletion is asked for, not 45 and 7 days on (checks.retention
              # reads both from the patched source)
              ../../../servers/photos/museum-delete-now.patch
              # each account's address kept beside it (<member>@users.<domain>,
              # a name), so a box with keys of its own carries the accounts over;
              # and an account deleted by museum itself when the box lists its
              # member as left, with no admin account (dd_jobs.go)
              ../../../servers/photos/museum-own-keys.patch
            ];
          });
          ente-web = prev.ente-web.overrideAttrs (
            o:
            let
              # Five apps are built from this one package (photos, accounts,
              # albums, cast, locker), each compiling its wasm and its whole
              # front end. What only Photos uses goes into Photos alone, so a
              # change to it - or to the bar - rebuilds one app, not five.
              photos = o.pname == "ente-web-photos";
            in
            {
              version = "1.3.36";
              src = prev.fetchFromGitHub {
                owner = "ente";
                repo = "ente";
                rev = "photos-v1.3.36";
                hash = "sha256-o75r8LFgG3BT3IIPiD9x6gY3fRDoxJ3ZTBPAYr3hLWI=";
              };
              # every app wears the site's colours, type and corners
              patches =
                (o.patches or [ ])
                ++ [ ../../../servers/photos/ente-web-theme.patch ]
                ++ lib.optionals photos [
                  # every way out of the app that would show ente's own sign-in
                  # or sign-up goes to the passkey page instead: the app is
                  # reached only through it
                  ../../../servers/photos/ente-web-passkey.patch
                  # the site's own bar across the top
                  ../../../servers/photos/ente-web-commonty.patch
                  # the menu: the site's, with a passkey where Ente asks for the
                  # password nobody here knows (the recovery key, the hidden
                  # section), and none of what changes it (account, plans)
                  ../../../servers/photos/ente-web-commonty-menu.patch
                  # pictures on screen fetched first, those flung past not at all
                  ../../../servers/photos/ente-web-commonty-thumbs.patch
                ];
              # Next names each build at random, and every page carries the
              # name: two builds of the same source never matched, and the
              # boxes vouch for a release by rebuilding it (dd-attest)
              postPatch =
                (o.postPatch or "")
                + ''
                  substituteInPlace packages/base/next.config.base.js \
                    --replace-fail 'output: "export",' "output: \"export\", generateBuildId: async () => \"commonty\","
                ''
                + lib.optionalString photos ''
                  # the bar's stylesheet is the site's own file, not a copy of it
                  cp ${../../../servers/web-shared/bar.css} apps/photos/src/styles/commonty-bar.css
                  # The grid drew three rows past the screen, and a picture is only
                  # fetched once it is drawn: scrolling reached pictures still on
                  # their way. Twenty rows is a few screens of warning.
                  substituteInPlace apps/photos/src/components/FileList.tsx \
                    --replace-fail "overscanCount={3}" "overscanCount={20}"
                '';
              # Next writes its build manifest as minified code whose short
              # names are handed out in whatever order its workers finished:
              # the same manifest, different bytes, and the boxes' rebuilds of
              # a release disagreed (dd-attest). Written back as the object it
              # makes, it comes out the same every time.
              postInstall = (o.postInstall or "") + ''
                find $out -name _buildManifest.js | while read -r f; do
                  ${prev.nodejs}/bin/node -e '
                    const fs = require("fs"), f = process.argv[1], self = {};
                    new Function("self", fs.readFileSync(f, "utf8"))(self);
                    fs.writeFileSync(f, "self.__BUILD_MANIFEST=" + JSON.stringify(self.__BUILD_MANIFEST) + ";self.__BUILD_MANIFEST_CB&&self.__BUILD_MANIFEST_CB();");
                  ' "$f"
                done
              '';
            }
          );
        })
      ];

      # the sessions made before the patch, hashed once (a token is never 64
      # hex characters, a hash always is), before museum reads the table
      systemd.services.ente.path = [ config.services.postgresql.package ];
      # its role and database are made by postgresql-setup: started together
      # by the plan on a box taking photos over, museum waits for them
      systemd.services.ente.after = [ "postgresql-setup.service" ];
      systemd.services.ente.wants = [ "postgresql-setup.service" ];
      systemd.services.ente.preStart = lib.mkAfter ''
        psql -v ON_ERROR_STOP=1 -q -d ente <<'SQL'
        DO $$ BEGIN IF to_regclass('public.tokens') IS NOT NULL THEN
          UPDATE tokens SET token = encode(sha256(convert_to(token,'UTF8')),'hex') WHERE token !~ '^[0-9a-f]{64}$';
        END IF; END $$;
        SQL
      '';

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
          OnActiveSec = "5min";
          OnUnitActiveSec = "3min";
        };
      };

      # this box's own keys, made once, before museum first starts here; and
      # its key to its own store's bucket, as museum reads it
      systemd.services.dd-ente-keys = lib.mkIf own {
        description = "Make museum's keys on this box";
        wantedBy = [ "ente.service" ];
        before = [ "ente.service" ];
        after = [ "dd-store-setup.service" ];
        requires = [ "dd-store-setup.service" ];
        path = [
          pkgs.openssl
          pkgs.coreutils
        ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
        };
        script = ''
          set -euo pipefail
          umask 077
          install -d -m 0750 -o ente -g dd-verify ${keys}
          make() { [ -s ${keys}/$1 ] || { "''${@:2}" > ${keys}/$1.new && mv ${keys}/$1.new ${keys}/$1; }; }
          make encryption openssl rand -base64 32
          hash64() { openssl rand 64 | base64 -w0; }
          jwt32() { openssl rand 32 | basenc --base64url -w0; }
          make hash hash64
          make jwt jwt32
          # the code the gate here sends with a member's own address
          make ott openssl rand -hex 24
          . /var/lib/dd-store/keys/ente.env
          printf %s "$AWS_ACCESS_KEY_ID" > ${keys}/s3-key
          printf %s "$AWS_SECRET_ACCESS_KEY" > ${keys}/s3-secret
          chown ente:ente ${keys}/encryption ${keys}/hash ${keys}/jwt ${keys}/s3-key ${keys}/s3-secret
          chmod 0400 ${keys}/encryption ${keys}/hash ${keys}/jwt ${keys}/s3-key ${keys}/s3-secret
          chown ente:dd-verify ${keys}/ott
          chmod 0440 ${keys}/ott
        '';
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
          # Museum listens on every address, its api and its metrics both, with
          # no setting to say otherwise: on loopback here, where nginx and
          # prometheus reach it. The firewall already stood in front of both;
          # this makes the box's own port list say what is true.
          package = pkgs.museum.overrideAttrs (o: {
            postPatch = (o.postPatch or "") + ''
              substituteInPlace cmd/museum/main.go \
                --replace-fail 'http.ListenAndServe(":2112", nil)' 'http.ListenAndServe("127.0.0.1:2112", nil)' \
                --replace-fail 'server.Run(fmt.Sprintf(":%d", port))' 'server.Run(fmt.Sprintf("127.0.0.1:%d", port))'
            '';
          });
          nginx.enable = true;
          enableLocalDB = true; # peer auth over a socket, so no db password exists
          domain = d "api";
          settings = {
            s3 = {
              use_path_style_urls = true;
              b2-eu-cen = {
                # browsers fetch and upload at this name: the pool's, or with
                # failover this box's own store, wherever photos runs
                endpoint = if own then "https://${d "photos-store"}" else "https://${d "s3"}";
                region = "us-east-1"; # required internally by ente regardless of reality
                bucket = "ente";
                key._secret = secret "s3-key" "garage-key-id";
                secret._secret = secret "s3-secret" "garage-key-secret";
              };
            };
            key = {
              encryption._secret = secret "encryption" "ente-key-encryption";
              hash._secret = secret "hash" "ente-key-hash";
            };
            jwt.secret._secret = secret "jwt" "ente-jwt-secret";
            # Addresses under users.<domain> are ours: a person's ente account
            # is <name>@users.<domain>, made by the photos page with this code
            # instead of a mail nobody would receive. Museum honours it because
            # the nixos module runs it as ENVIRONMENT=local.
            # nobody may call museum's admin api: what the fleet needs of it
            # the box does in museum itself (storage/ledger.nix, dd_jobs.go)
            internal.admins = [ ];
            internal.hardcoded-ott = {
              local-domain-suffix = "@users.${base}";
              local-domain-value._secret = secret "ott" "ente-ott";
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
          ${d "api"} = {
            useACMEHost = base;
            forceSSL = true;
            # nobody is an admin; the admin api answers nobody either
            locations."^~ /admin".return = "404";
          };
        }
        // {
          # The photos page makes or links a person's account against museum
          # on its own name: same origin, so the member's session comes along.
          # Museum as it is, but for the two calls that carry the fleet's
          # verification code, which the gate makes with the code filled in
          # (servers/gate/src/photos.rs, museum_verify).
          ${d "photos"} = {
            useACMEHost = base;
            forceSSL = true;
            # the ente module names albums here too; albums has its own
            # server above, and nginx would answer it from the first anyway
            serverAliases = lib.mkForce [ ];
            locations = {
              # Ente's client holds a person's keys, so it comes with the app
              # (app/src/local.rs) and is never served from here: a browser is
              # told to open Photos in the app. The root stays set, as the
              # app's build takes the client from it (flake.nix appWeb).
              "/".extraConfig = ''
                error_page 418 = @use-app;
                return 418;
              '';
              "@use-app" = {
                proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}";
                extraConfig = "rewrite ^ /_dd/use-app break;";
              };
              "/_dd/museum/".proxyPass = "http://127.0.0.1:${toString museumPort}/";
              # nobody is an admin; the admin api answers nobody either
              "^~ /_dd/museum/admin".return = "404";
              "~ ^/_dd/museum/users/(verify-email|change-email)$".proxyPass =
                "http://127.0.0.1:${toString config.dd.verify.port}/_dd/photos/museum/$1";
            };
          };
          ${d "locker"} = {
            useACMEHost = base;
            forceSSL = true;
            locations."/" = {
              root = lockerPkg;
              tryFiles = "$uri $uri.html /index.html";
            };
          };
        };
    }
  ];
}
