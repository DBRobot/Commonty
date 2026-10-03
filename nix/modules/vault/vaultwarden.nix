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

  # Pinned: a nixpkgs bump that moves Vaultwarden stops here until someone
  # reads what changed and updates this line.
  version = "1.37.1";
  vaultwarden =
    assert lib.assertMsg (pkgs.vaultwarden.version == version)
      "vaultwarden is ${pkgs.vaultwarden.version} in nixpkgs, pinned to ${version}: check the release notes, then update modules/vault/vaultwarden.nix";
    (pkgs.vaultwarden.override { dbBackend = "postgresql"; }).overrideAttrs (o: {
      # refresh tokens kept as sha256: a copy of the database renews nobody
      patches = (o.patches or [ ]) ++ [ ../../../box/vault/vaultwarden-hash-refresh.patch ];
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

  # The web vault asks the breach list on this box (pwned.nix) instead of
  # api.pwnedpasswords.com: checking a password never leaves the box.
  webvault =
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
      ! grep -q 'Vaultwarden Web' $v/index.html
      # master passwords from 8 characters, as Bitwarden had them before
      # 12 (David's call, 2026-09-28): the check is the client's, here
      grep -rl --include='*.js' 'minimumPasswordLength=12' $out/share/vaultwarden/vault | while read -r f; do
        sed -i 's|minimumPasswordLength=12|minimumPasswordLength=8|g' "$f"
      done
      ! grep -rq --include='*.js' 'minimumPasswordLength=12' $out/share/vaultwarden/vault
    ''
    // {
      inherit (pkgs.vaultwarden.webvault) version;
    };

  # Commonty's look, through Vaultwarden's own hook for it: a stylesheet
  # template it serves with the web vault. Their pages, our colours and type.
  templates = pkgs.runCommand "dd-vault-templates" { } ''
    mkdir -p $out/scss
    cp ${../../../box/vault/web/user.vaultwarden.scss.hbs} $out/scss/user.vaultwarden.scss.hbs
  '';

in
{
  imports = [
    ./pwned.nix
    ../member-mail.nix
  ];

  options.dd.vault = {
    enable = lib.mkEnableOption "Passwords (Vaultwarden) on this box";
    envFile = lib.mkOption {
      type = lib.types.path;
      description = "env file with its garage key as VAULT_ID/VAULT_SECRET and again as AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY, and SSO_CLIENT_SECRET";
    };
    issuer = lib.mkOption {
      type = lib.types.str;
      default = "https://home.${config.dd.domain}/_dd/oidc";
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
      {
        # what the box holds: every vault encrypted on its owner's devices; names,
        # who is in the Family and when things changed are in the clear. The web
        # vault's code comes from here, so a box that turned hostile could serve a
        # page that reads a master password as it is typed: label it as such.
        dd.box.plaintext = [ "vaultwarden (serves the page master passwords are typed into)" ];

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
            DATA_FOLDER = "s3://vaultwarden/data?endpoint=http://127.0.0.1:3900&region=us-east-1&enable_virtual_host_style=false&default_storage_class=STANDARD";
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

            LOG_LEVEL = "warn";
            EXTENDED_LOGGING = true;
          };
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
          after = [ "garage-setup.service" ];
          wants = [ "garage-setup.service" ];
          environment.AWS_REGION = "us-east-1";
          serviceConfig.LoadCredential = config.dd.memberMail.credentials;
        };

        # Signing a member out of Passwords: the gate drops their name in
        # /run/dd-ended when one of their devices or passkeys is removed, or
        # they sign out everywhere else (box/verify/src/directory.rs). Their
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

        # the gate: the issuer this sign-in goes through (box/verify/src/oidc.rs)
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

        # the vault's records, in the hourly dumps the backups ship
        services.postgresqlBackup.databases = [ "vaultwarden" ];

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
              # inside: the web vault, with the Commonty bar added to its page;
              # outside: the Send page and nothing else of it
              "/" = toVault (
                lib.optionalString public ''
                  error_page 418 = @send;
                  if ($dd_inside = 0) { return 418; }
                ''
                + ''
                  proxy_set_header Accept-Encoding "";
                  sub_filter_once on;
                  sub_filter_types text/html;
                  sub_filter '</head>' '<link rel="stylesheet" href="/_dd/static/bar.css"><link rel="stylesheet" href="/_dd/static/vault-bar.css"><script type="module" src="/_dd/static/vault-bar.js"></script></head>';
                ''
              );
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
