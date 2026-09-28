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
  issuer = "https://home.${base}/_dd/oidc";

  # Pinned: a nixpkgs bump that moves Vaultwarden stops here until someone
  # reads what changed and updates this line.
  version = "1.37.1";
  vaultwarden =
    assert lib.assertMsg (pkgs.vaultwarden.version == version)
      "vaultwarden is ${pkgs.vaultwarden.version} in nixpkgs, pinned to ${version}: check the release notes, then update modules/vault/vaultwarden.nix";
    (pkgs.vaultwarden.override { dbBackend = "postgresql"; }).overrideAttrs (_: {
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
          after = [ "garage-setup.service" ];
          wants = [ "garage-setup.service" ];
          environment.AWS_REGION = "us-east-1";
          serviceConfig.LoadCredential = config.dd.memberMail.credentials;
        };

        # the gate: the issuer this sign-in goes through (box/verify/src/oidc.rs)
        systemd.services.dd-verify = {
          environment = {
            VERIFY_OIDC_ISSUER = issuer;
            VERIFY_OIDC_CLIENT_ID = "vaultwarden";
            VERIFY_OIDC_CLIENT_SECRET_FILE = "/run/credentials/dd-verify.service/oidc-secret";
            VERIFY_OIDC_REDIRECT = "https://${host}/identity/connect/oidc-signin";
          };
          serviceConfig.LoadCredential = [ "oidc-secret:${cfg.oidcSecretFile}" ];
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
                extraConfig = ''
                  proxy_set_header Cookie $dd_cookie_stripped;
                ''
                + extra;
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
              "/_dd/" = {
                proxyPass = "${gate}/_dd/";
                extraConfig = inside + ''
                  proxy_set_header X-Original-URI $request_uri;
                '';
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
