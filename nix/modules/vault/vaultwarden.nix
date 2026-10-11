{
  config,
  options,
  pkgs,
  lib,
  ...
}:
let
  base = config.dd.domain;
  host = "vault.${base}";
  port = 8222;
  cfg = config.dd.vault;
  # the front door is the gateway role's; a box without it is network-only
  hasPublic = options.dd ? public;
  public = hasPublic && config.dd.public.enable;
  issuer = cfg.issuer;
  homeIssuer = "https://home.${base}/_dd/oidc";

  # Pinned: a nixpkgs bump that moves Vaultwarden stops here until someone
  # reads what changed and updates this line.
  version = "1.37.1";
  vaultwarden =
    assert lib.assertMsg (pkgs.vaultwarden.version == version)
      "vaultwarden is ${pkgs.vaultwarden.version} in nixpkgs, pinned to ${version}: check the release notes, then update modules/vault/vaultwarden.nix";
    (pkgs.vaultwarden.override { dbBackend = "postgresql"; }).overrideAttrs (o: {
      # refresh tokens kept as sha256: a copy of the database renews nobody
      patches = (o.patches or [ ]) ++ [
        ../../../servers/vault/vaultwarden-hash-refresh.patch
        # the Commonty app's web vault signs in on this computer's loopback
        ../../../servers/vault/vaultwarden-app-loopback.patch
      ];
      # files (attachments, sends) in garage, not on this laptop's disk.
      # buildRustPackage turns buildFeatures into these when the package is
      # made, so an override has to set them directly
      cargoBuildFeatures = [
        "postgresql"
        "s3"
      ];
      cargoCheckFeatures = [
        "postgresql"
        "s3"
      ];
    });

  # The web vault, pinned like the server: its edits below match its text,
  # so a nixpkgs bump that moves it stops here until they are checked.
  webvaultVersion = "2026.6.4+0";

  # The web vault asks the breach list on this box (pwned.nix) instead of
  # api.pwnedpasswords.com: checking a password never leaves the box.
  webvault =
    assert lib.assertMsg (pkgs.vaultwarden.webvault.version == webvaultVersion)
      "vaultwarden's web vault is ${pkgs.vaultwarden.webvault.version} in nixpkgs, pinned to ${webvaultVersion}: check the edits in modules/vault/vaultwarden.nix still match, then update it";
    pkgs.runCommand "vaultwarden-webvault-local-breach-list" { } ''
      cp -r ${pkgs.vaultwarden.webvault} $out
      chmod -R u+w $out
      hits=$(grep -rl --include='*.js' 'https://api.pwnedpasswords.com/range/' $out/share/vaultwarden/vault)
      [ -n "$hits" ] || { echo "the web vault no longer asks api.pwnedpasswords.com; check pwned.nix" >&2; exit 1; }
      for f in $hits; do
        sed -i 's|https://api.pwnedpasswords.com/range/|/pwned/range/|g' "$f"
      done
      # Commonty's own name and mark on every page. Bitwarden's name, its
      # vault safe and its favicon on a login page at a domain that is not
      # Bitwarden's is what browsers' phishing checks look for, and Chrome
      # flagged commonty.org for it the night Passwords went live.
      v=$out/share/vaultwarden/vault
      sed -i 's|<title page-title>Vaultwarden Web</title>|<title page-title>Commonty Passwords</title>|; s|content="#175DDC"|content="#1D5C42"|g; s|color="#175DDC"|color="#1D5C42"|g' $v/index.html
      for f in $(grep -rl --include='*.js' 'Vaultwarden Web' $v); do sed -i 's|Vaultwarden Web|Commonty Passwords|g' "$f"; done
      for f in $(grep -rl --include='*.js' 'A modified version of the Bitwarden® Web Vault for Vaultwarden (an unofficial rewrite of the Bitwarden® server).' $v); do
        sed -i 's|A modified version of the Bitwarden® Web Vault for Vaultwarden (an unofficial rewrite of the Bitwarden® server).|Built on Vaultwarden, open-source.|g' "$f"
      done
      for f in $v/*.json; do sed -i 's|"Vaultwarden Web"|"Commonty Passwords"|g; s|"Vaultwarden"|"Commonty Passwords"|g' "$f"; done
      ${pkgs.imagemagick}/bin/magick ${../../../app/icons/icon.png} -resize 32x32 $v/images/favicon-32x32.png
      ${pkgs.imagemagick}/bin/magick ${../../../app/icons/icon.png} -resize 16x16 $v/images/favicon-16x16.png
      ${pkgs.imagemagick}/bin/magick ${../../../app/icons/icon.png} -resize 180x180 $v/images/apple-touch-icon.png
      # a `!` command never stops the build under set -e: these test and exit
      if grep -q 'Vaultwarden Web' $v/index.html; then echo "the web vault's title moved" >&2; exit 1; fi
      # master passwords from 8 characters, as Bitwarden had them before
      # 12 (David's call, 2026-09-28): the check is the client's, here
      hits=$(grep -rl --include='*.js' 'minimumPasswordLength=12' $v || true)
      [ -n "$hits" ] || { echo "the web vault's minimumPasswordLength=12 moved; find the new spot" >&2; exit 1; }
      for f in $hits; do
        sed -i 's|minimumPasswordLength=12|minimumPasswordLength=8|g' "$f"
      done
      if grep -rq --include='*.js' 'minimumPasswordLength=12' $v; then exit 1; fi
    ''
    // {
      inherit (pkgs.vaultwarden.webvault) version;
    };

  # Commonty's look, through Vaultwarden's own hook for it: a stylesheet
  # template it serves with the web vault. Their pages, our colours and type.
  templates = pkgs.runCommand "dd-vault-templates" { } ''
    mkdir -p $out/scss
    cp ${../../../servers/vault/web/user.vaultwarden.scss.hbs} $out/scss/user.vaultwarden.scss.hbs
  '';

in
{
  imports = [
    ./pwned.nix
    ../member-mail.nix
    ../storage/usage.nix
    ../storage/postgres-backup.nix
  ];

  options.dd.vault = {
    enable = lib.mkEnableOption "Passwords (Vaultwarden) on this box";
    envFile = lib.mkOption {
      type = lib.types.path;
      description = "env file with its garage key as VAULT_ID/VAULT_SECRET and again as AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY, and SSO_CLIENT_SECRET";
    };
    # The issuer is this box's own gate (servers/gate/src/oidc.rs): the
    # sessions it reads are this box's alone (session.rs), and it gets the
    # client's settings below. It is reached at home.<domain>, which the
    # fleet's names point at the gateway, so Passwords runs on the gateway
    # (fleet/roles.json: with gateway; asserted below).
    issuer = lib.mkOption {
      type = lib.types.str;
      default = homeIssuer;
      description = "the gate's sign-in issuer; a test points it at the gate itself";
    };
    oidcSecretFile = lib.mkOption {
      type = lib.types.path;
      description = "the sign-in secret the gate and Vaultwarden share, for the gate";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      (lib.optionalAttrs hasPublic { dd.public.hosts = [ "vault" ]; })
      # the bucket and its key are made by the garage on this box, when there
      # is one; a vault on a box without garage needs them made already
      (lib.optionalAttrs (options.dd ? garage) {
        dd.garage.setupEnvFiles = [ cfg.envFile ];
        dd.garage.buckets.vaultwarden = {
          key = {
            name = "dd-vault";
            envPrefix = "VAULT";
          };
          allow = [
            "read"
            "write"
          ];
        };
      })
      {
        assertions = [
          {
            assertion = issuer != homeIssuer || public;
            message = "Passwords signs in through home.${base}, which is the gateway's gate: run the vault role on the gateway, or set dd.vault.issuer to this box's own gate";
          }
        ];

        # what the box holds: every vault encrypted on its owner's devices; names,
        # who is in the Family and when things changed are in the clear. The web
        # vault's code comes from here, so a box that turned hostile could serve a
        # page that reads a master password as it is typed: label it as such.
        dd.box.plaintext = [ "vaultwarden (serves the page master passwords are typed into)" ];

        services.vaultwarden = {
          enable = true;
          package = vaultwarden;
          webVaultPackage = webvault;
          dbBackend = "postgresql";
          configurePostgres = true;
          environmentFile = [ cfg.envFile ];
          config = {
            DOMAIN = "https://${host}";
            ROCKET_ADDRESS = "127.0.0.1";
            ROCKET_PORT = port;
            # the storage pool, wherever it is: this box's garage or a
            # storage box's over the tailnet
            DATA_FOLDER = "s3://vaultwarden/data?endpoint=${config.dd.box.s3}&region=us-east-1&enable_virtual_host_style=false&default_storage_class=STANDARD";
            TMP_FOLDER = "/var/lib/vaultwarden/tmp";
            TEMPLATES_FOLDER = "${templates}";

            # members only, and only through Commonty: the gate signs them in
            SSO_ENABLED = true;
            SSO_ONLY = true;
            SSO_AUTHORITY = issuer;
            SSO_CLIENT_ID = "vaultwarden";
            SSO_PKCE = true;
            SSO_SCOPES = "email profile";
            SSO_SIGNUPS_MATCH_EMAIL = true;
            # vaultwarden keeps its own session once someone is in
            SSO_AUTH_ONLY_NOT_SESSION = true;
            SIGNUPS_ALLOWED = true; # an account is made at a member's first sign-in
            SIGNUPS_DOMAINS_WHITELIST = base;
            SIGNUPS_VERIFY = false;
            EMAIL_CHANGE_ALLOWED = false;

            INVITATIONS_ALLOWED = true;
            INVITATION_ORG_NAME = "Commonty";
            EMERGENCY_ACCESS_ALLOWED = true;
            SENDS_ALLOWED = true;
            ORG_EVENTS_ENABLED = true;
            PASSWORD_HINTS_ALLOWED = true;
            SHOW_PASSWORD_HINT = false;

            # no site icons: fetching them would tell the box which sites are in
            # a vault
            ICON_SERVICE = "internal";
            DISABLE_ICON_DOWNLOAD = true;

            USE_SENDMAIL = true;
            SENDMAIL_COMMAND = "${config.dd.memberMail.sendmail}";
            SMTP_FROM = "distributed.datacenter@gmail.com";
            SMTP_FROM_NAME = "Commonty Passwords";

            # nothing deleted lasts past 90 days (nix/tests/retention.nix);
            # upstream keeps both forever
            TRASH_AUTO_DELETE_DAYS = 90;
            EVENTS_DAYS_RETAIN = 90;

            LOG_LEVEL = "warn";
            EXTENDED_LOGGING = true;
          };
        };

        # each member's attachments, counted by the vault itself (storage/usage.nix)
        dd.usage.vaultwarden = {
          user = "vaultwarden";
          database = "vaultwarden";
          query = "select u.email, coalesce(sum(a.file_size), 0) from attachments a join ciphers c on c.uuid = a.cipher_uuid join users u on u.uuid = c.user_uuid group by u.email";
          counts = "passwords";
          memberSuffix = "@${config.dd.domain}";
        };

        systemd.services.vaultwarden = {
          path = [ config.services.postgresql.package ];
          # the refresh tokens made before the patch, hashed once
          preStart = ''
            psql -v ON_ERROR_STOP=1 -q -d vaultwarden <<'SQL'
            DO $$ BEGIN IF to_regclass('public.devices') IS NOT NULL THEN
              UPDATE devices SET refresh_token = encode(sha256(convert_to(refresh_token,'UTF8')),'hex') WHERE refresh_token !~ '^[0-9a-f]{64}$';
            END IF; END $$;
            SQL
          '';
          after = lib.optional (options.dd ? garage) "garage-setup.service";
          wants = lib.optional (options.dd ? garage) "garage-setup.service";
          environment.AWS_REGION = "us-east-1";
          serviceConfig.LoadCredential = config.dd.memberMail.credentials;
        };

        # Signing a member out of Passwords: the gate drops their name in
        # /run/dd-ended when one of their devices or passkeys is removed, or
        # they sign out everywhere else (servers/gate/src/directory.rs). Their
        # apps and browsers here are forgotten, as the admin page's
        # "deauthorize" does, and they sign in again through the gate. The
        # gate never touches the vault's database; this runs as the vault.
        systemd.tmpfiles.rules = [ "d /run/dd-ended 0770 dd-verify vaultwarden -" ];
        systemd.paths.dd-vault-end = {
          wantedBy = [ "paths.target" ];
          pathConfig.DirectoryNotEmpty = "/run/dd-ended";
        };
        systemd.services.dd-vault-end = {
          after = [ "postgresql.service" ];
          path = [ config.services.postgresql.package ];
          serviceConfig = {
            Type = "oneshot";
            User = "vaultwarden";
            Group = "vaultwarden";
          };
          script = ''
            for f in /run/dd-ended/*; do
              [ -e "$f" ] || continue
              name=$(basename "$f")
              rm -f -- "$f"
              case "$name" in
                *[!a-z0-9._-]* | [!a-z0-9]*) echo "not a name: skipped"; continue ;;
              esac
              psql -v ON_ERROR_STOP=1 -q -d vaultwarden -v email="$name@${base}" <<'SQL'
            BEGIN;
            DELETE FROM devices WHERE user_uuid IN (SELECT uuid FROM users WHERE lower(email) = lower(:'email'));
            UPDATE users SET security_stamp = gen_random_uuid()::text, stamp_exception = NULL, updated_at = now()
              WHERE lower(email) = lower(:'email');
            COMMIT;
            SQL
              echo "$name: signed out of Passwords everywhere"
            done
          '';
        };

        # the gate: the issuer this sign-in goes through (servers/gate/src/oidc.rs)
        systemd.services.dd-verify = {
          environment = {
            VERIFY_OIDC_ISSUER = issuer;
            VERIFY_OIDC_CLIENT_ID = "vaultwarden";
            VERIFY_OIDC_CLIENT_SECRET_FILE = "/run/credentials/dd-verify.service/oidc-secret";
            VERIFY_OIDC_REDIRECT = "https://${host}/identity/connect/oidc-signin";
          };
          environment.VERIFY_ENDED = "/run/dd-ended";
          serviceConfig.LoadCredential = [ "oidc-secret:${cfg.oidcSecretFile}" ];
          serviceConfig.ReadWritePaths = [ "/run/dd-ended" ];
        };

        # Only on the Commonty network - apps reach it through the phone's or
        # laptop's tailnet - except what a Send needs: someone outside opening a
        # link gets the Send page and the calls it makes, and nothing else.
        services.nginx.virtualHosts.${host} = {
          useACMEHost = base;
          forceSSL = true;
          locations =
            let
              inside = lib.optionalString public ''
                if ($dd_inside = 0) { return 444; }
              '';
              gate = "http://127.0.0.1:${toString config.dd.verify.port}";
              # the gate's session cookie is for the gate: Vaultwarden never sees it
              toVault = extra: {
                proxyPass = "http://127.0.0.1:${toString port}";
                proxyWebsockets = true;
                extraConfig = extra;
              };
              # quoted where used: nginx reads a bare { in a location as a block
              uuid = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
            in
            {
              # The web vault holds a person's keys, so it comes with the app
              # (app/src/local.rs) and is never served from here. Inside, a
              # browser is told to open Passwords in the app; the same page
              # starts a Bitwarden app's sign-in and opens a Send (use-app.js).
              # Outside: the Send page and nothing else.
              "/" = {
                extraConfig =
                  lib.optionalString public ''
                    error_page 418 = @send;
                    if ($dd_inside = 0) { return 418; }
                  ''
                  + ''
                    error_page 419 = @use-app;
                    return 419;
                  '';
              };
              "@use-app" = {
                proxyPass = gate;
                extraConfig = ''
                  rewrite ^ /_dd/use-app break;
                '';
              };
              # a Send opened from inside: use-app.js sends it here
              "= /send" = {
                proxyPass = gate;
                extraConfig = ''
                  rewrite ^ /_dd/send break;
                '';
              };
              "@send" = {
                proxyPass = gate;
                extraConfig = ''
                  rewrite ^ /_dd/send break;
                '';
              };
              "/api/" = toVault (
                inside
                + ''
                  client_max_body_size 525M;
                ''
              );
              "/identity/" = toVault inside;
              "/notifications/" = toVault inside;
              # what the app's own web vault asks besides (app/src/local.rs);
              # "/" no longer reaches Vaultwarden
              "/events/" = toVault inside;
              "= /alive" = toVault inside;
              # what a Send link needs from outside: open it, ask for its file,
              # fetch the file (ciphertext; the key is in the link's #, which
              # never reaches a server)
              "~ \"^/api/sends/(access/[A-Za-z0-9_-]+|${uuid}/access/file/[a-z0-9]+)$\"" = toVault ''
                limit_except POST { deny all; }
              '';
              "~ \"^/api/sends/${uuid}/[a-z0-9]+$\"" = toVault ''
                limit_except GET { deny all; }
              '';
              # the bar's and the Send page's files, and inside, /_dd/me for the bar
              "/_dd/static/" = {
                proxyPass = "${gate}/_dd/static/";
              };
              # what the bar above the vault asks the gate: who is looking, and
              # the menu's switch. The gate's pages are the home site's: any
              # other /_dd/ address here goes there, so nobody browses the
              # whole site under vault.
              "~ ^/_dd/(me|adblock/state|adblock/switch)$" = {
                proxyPass = gate;
                extraConfig = inside + ''
                  proxy_set_header X-Original-URI $request_uri;
                '';
              };
              "/_dd/" = {
                return = "302 https://home.${base}$request_uri";
              };
              # never fetched: a request for one would still tell the box a site
              "/icons/" = {
                return = "404";
                extraConfig = "access_log off;";
              };
            };
        };
      }
    ]
  );
}
