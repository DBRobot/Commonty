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
  # with failover on (box/box.nix): its sign-in secret is made here for the
  # gate here, and its files are in this box's own store
  own = config.dd.failover or false;
  keys = "/var/lib/dd-vault-keys";

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
      # the site's icon, and its mark beside "Passwords" where the
      # Vaultwarden and Bitwarden logos were; the bitmaps drawn from them here
      fav=${../../../servers/web-shared/favicon.svg}
      cp $fav $v/images/favicon.svg
      cp $fav $v/images/icon-white.svg
      cp $fav $v/images/safari-pinned-tab.svg
      cp $fav $v/images/icons/safari-pinned-tab.svg
      cp ${../../../servers/vault/web/logo.svg} $v/images/logo.svg
      cp ${../../../servers/vault/web/logo-white.svg} $v/images/logo-white.svg
      export XDG_CACHE_HOME=$TMPDIR FONTCONFIG_FILE=${
        pkgs.makeFontsConf { fontDirectories = [ pkgs.public-sans ]; }
      }
      png() { ${pkgs.librsvg}/bin/rsvg-convert "$@"; }
      for s in 16 32; do png -w $s -h $s $fav -o $v/images/favicon-''${s}x$s.png; cp $v/images/favicon-''${s}x$s.png $v/images/icons/; done
      for f in apple-touch-icon:180 android-chrome-192x192:192 android-chrome-512x512:512 mstile-150x150:270; do
        png -w ''${f#*:} -h ''${f#*:} $fav -o $v/images/''${f%:*}.png
        cp $v/images/''${f%:*}.png $v/images/icons/
      done
      png -w 32 -h 32 $fav -o $v/images/icon-dark.png
      cp $v/images/icon-dark.png $v/images/icon-white.png
      png -h 86 $v/images/logo.svg -o $v/images/logo-dark@2x.png
      png -h 86 $v/images/logo-white.svg -o $v/images/logo-white@2x.png
      ${pkgs.imagemagick}/bin/magick $v/images/favicon-16x16.png $v/images/favicon-32x32.png -strip $v/favicon.ico
      # every page's tab: the site's icon, and Passwords where it said Bitwarden
      for f in $v/*.html; do
        sed -i -E 's#<link rel="icon" type="image/png" sizes="32x32" href="images/favicon-32x32.png"/?>#<link rel="icon" type="image/svg+xml" href="images/favicon.svg"/>#; s#<title>([^<]*)Bitwarden( Web vault)?([^<]*)</title>#<title>\1Commonty Passwords\3</title>#; s#alt="(Bitwarden|Vaultwarden)"#alt="Passwords"#g' "$f"
      done
      grep -q 'images/favicon.svg' $v/index.html || { echo "the web vault's icon links moved" >&2; exit 1; }
      if grep -q '<title>[^<]*Bitwarden' $v/*.html; then echo "a page's title still says Bitwarden" >&2; exit 1; fi
      for f in $(grep -rl --include='*.js' '"alt","Bitwarden"' $v); do sed -i 's|"alt","Bitwarden"|"alt","Passwords"|g' "$f"; done
      for f in $(grep -rl --include='*.js' '"alt","Vaultwarden"' $v); do sed -i 's|"alt","Vaultwarden"|"alt","Passwords"|g' "$f"; done
      # what an authenticator app names the account its code is for
      for f in $(grep -rl --include='*.js' 'otpauth://totp/Vaultwarden:' $v); do
        sed -i 's|otpauth://totp/Vaultwarden:|otpauth://totp/Commonty:|g; s|&issuer=Vaultwarden|\&issuer=Commonty|g' "$f"
      done
      # the words on the sign-in and welcome pages, in English: the rest
      # still say Bitwarden where they mean Bitwarden's own apps
      for l in en en_GB en_IN; do
        ${pkgs.jq}/bin/jq --indent 2 '
          def set($k; $m): if has($k) then .[$k].message = $m else error("no \($k) in messages.json") end;
          set("loginPageEmailEntryScreenTitle"; "Log in to Passwords")
          | set("bitWebVault"; "Passwords")
          | set("bitwardenPasswordManager"; "Passwords")
          | set("vaultWelcomeDialogTitle"; "You'"'"'re in! Welcome to Passwords")
          | set("vaultWelcomeDialogDescription"; "Store all your passwords and personal info in Passwords. We'"'"'ll show you around.")
          | set("individualUpgradeWelcomeMessage"; "Welcome to Passwords")
        ' $v/locales/$l/messages.json > messages.json
        mv messages.json $v/locales/$l/messages.json
      done
      # Vaultwarden renders /css/vaultwarden.css for every request, but the
      # app serves this web vault itself and runs no code a box sends: the
      # same stylesheet, made here from its template with this server's
      # settings (and grass, the compiler Vaultwarden uses)
      mkdir -p $v/css $v/fonts
      cp ${../../../servers/web-shared/fonts/public-sans.woff2} $v/fonts/public-sans.woff2
      ${
        pkgs.python3.withPackages (p: [ p.pybars3 ])
      }/bin/python3 -I ${../../../servers/vault/web/vaultwarden-css.py} \
        ${vaultwarden.src}/src/static/templates/scss/vaultwarden.scss.hbs \
        ${../../../servers/vault/web/user.vaultwarden.scss.hbs} \
        ${pkgs.writeText "vaultwarden-css.json" (builtins.toJSON cssSettings)} \
        ${pkgs.vaultwarden.webvault.version} > vaultwarden.scss
      ${pkgs.grass-sass}/bin/grass -s compressed vaultwarden.scss $v/css/vaultwarden.css
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

  # what Vaultwarden's own /css/vaultwarden.css is rendered with (its
  # src/api/web.rs), from this server's settings, for the copy made below
  vw = config.services.vaultwarden.config;
  cssSettings = {
    load_user_scss = true;
    emergency_access_allowed = vw.EMERGENCY_ACCESS_ALLOWED;
    mail_2fa_enabled = vw.USE_SENDMAIL;
    sends_allowed = vw.SENDS_ALLOWED;
    remember_2fa_disabled = false;
    password_hints_allowed = vw.PASSWORD_HINTS_ALLOWED;
    signup_disabled = vw.SSO_ENABLED && vw.SSO_ONLY;
    sso_enabled = vw.SSO_ENABLED;
    sso_only = vw.SSO_ENABLED && vw.SSO_ONLY;
    webauthn_2fa_supported = true;
    yubico_enabled = false;
  };

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
    # (nix/roles/bundles.json: with gateway; asserted below).
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
      (lib.optionalAttrs (options.dd ? garage) (
        lib.mkIf (!own) {
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
        }
      ))
      (lib.mkIf own {
        # started with postgres by the plan: its role and database first
        systemd.services.vaultwarden.after = [ "postgresql-setup.service" ];
        systemd.services.vaultwarden.wants = [ "postgresql-setup.service" ];
        # postgres may be up after this starts: its lease or its restore can come
        # later than this one's. Retried until it is, never a failed unit
        # (2026-10-24: museum gave up after five tries and Photos stayed down)
        systemd.services.vaultwarden.startLimitIntervalSec = 0;
        systemd.services.vaultwarden.serviceConfig = {
          RestartSec = 2;
          RestartSteps = 5;
          RestartMaxDelaySec = 30;
        };
        dd.vault.envFile = "${keys}/env";
        dd.vault.oidcSecretFile = "${keys}/oidc";
        systemd.services.dd-vault-keys = {
          description = "Make the vault's sign-in secret on this box";
          wantedBy = [ "vaultwarden.service" ];
          before = [
            "vaultwarden.service"
            "dd-verify.service"
          ];
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
            install -d -m 0750 -o root -g dd-verify ${keys}
            [ -s ${keys}/oidc ] || { openssl rand -hex 32 > ${keys}/oidc.new && mv ${keys}/oidc.new ${keys}/oidc; }
            chown root:dd-verify ${keys}/oidc
            chmod 0440 ${keys}/oidc
            . /var/lib/dd-store/keys/vaultwarden.env
            {
              echo "AWS_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID"
              echo "AWS_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY"
              echo "SSO_CLIENT_SECRET=$(cat ${keys}/oidc)"
            } > ${keys}/env.new
            chmod 0400 ${keys}/env.new
            mv ${keys}/env.new ${keys}/env
          '';
        };
      })
      {
        assertions = [
          {
            # its files are in this box's own store, which it reaches with
            # the store's key only with failover on: the old shared pool,
            # where they were before, is gone
            assertion = own || config.dd.box.s3 != null;
            message = "Passwords keeps its files in the box's own store, and its key there comes with failover: turn failover on (fleet/policy.json)";
          }
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
          environmentFile = [
            cfg.envFile
            # ADMIN_TOKEN, for dd-forget-passwords below
            "/run/dd-vault-admin/env"
          ];
          config = {
            DOMAIN = "https://${host}";
            ROCKET_ADDRESS = "127.0.0.1";
            ROCKET_PORT = port;
            # this box's own store, copied to others' where the plan says
            DATA_FOLDER = "s3://vaultwarden/data?endpoint=${
              if own then
                "http://127.0.0.1:${toString config.dd.store.port}"
              else if config.dd.box.s3 != null then
                config.dd.box.s3
              else
                throw "Passwords keeps its files in the box's own store, and its key there comes with failover: turn failover on (fleet/policy.json)"
            }&region=us-east-1&enable_virtual_host_style=false&default_storage_class=STANDARD";
            # the key its sessions are signed with: this box's, on its own
            # disk, never in the bucket that is copied to other boxes. A box
            # taking over makes its own; members sign in again, through SSO
            RSA_KEY_FILENAME = "/var/lib/vaultwarden/rsa_key";
            TMP_FOLDER = "/var/lib/vaultwarden/tmp";

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
            PASSWORD_HINTS_ALLOWED = false;
            SHOW_PASSWORD_HINT = false;

            # no site icons: fetching them would tell the box which sites are in
            # a vault
            ICON_SERVICE = "internal";
            DISABLE_ICON_DOWNLOAD = true;

            USE_SENDMAIL = true;
            SENDMAIL_COMMAND = config.dd.mail.sendmail;
            # the mail Worker sends it as noreply@<domain> whatever it says here
            SMTP_FROM = "noreply@${config.dd.domain}";
            SMTP_FROM_NAME = "Commonty Passwords";

            # a deleted item is gone everywhere within 90 days (nix/tests/retention.nix);
            # upstream keeps both forever
            TRASH_AUTO_DELETE_DAYS = 30;
            EVENTS_DAYS_RETAIN = 30;

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
          serviceConfig.SupplementaryGroups = [ config.dd.mail.group ];
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

        # Deleting the account of someone who left is upstream's
        # own admin delete (sends, attachments in the bucket, everything), on
        # loopback only: nginx never passes /admin, and the token is made at
        # boot, kept in /run, readable by root alone (docs/dev/leaving.md)
        systemd.services.dd-vault-admin-token = {
          description = "A fresh admin token for Vaultwarden, for this boot";
          restartIfChanged = false;
          before = [ "vaultwarden.service" ];
          requiredBy = [ "vaultwarden.service" ];
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
          };
          script = ''
            install -d -m 0700 /run/dd-vault-admin
            [ -s /run/dd-vault-admin/env ] && exit 0
            umask 077
            printf 'ADMIN_TOKEN=%s\n' "$(${pkgs.openssl}/bin/openssl rand -hex 32)" > /run/dd-vault-admin/env.tmp
            mv /run/dd-vault-admin/env.tmp /run/dd-vault-admin/env
          '';
        };
        systemd.services.dd-forget-passwords = {
          description = "Delete the Passwords accounts of members who left";
          after = [
            "vaultwarden.service"
            "dd-verify.service"
          ];
          restartIfChanged = false;
          path = [
            pkgs.curl
            pkgs.jq
            pkgs.gnugrep
          ];
          serviceConfig = {
            Type = "oneshot";
            PrivateTmp = true;
          };
          # a service not answering is next time, never a failed unit
          script = ''
            set +e
            gate=http://127.0.0.1:${toString config.dd.verify.port}
            vw=http://127.0.0.1:${toString port}
            list=$(curl -fsS "$gate/internal/leaving") || { echo "the gate is not answering; next time"; exit 0; }
            due=$(echo "$list" | jq -r '.members[] | select(.due and ((.done // []) | index("passwords") | not)) | .name')
            [ -n "$due" ] || exit 0
            . /run/dd-vault-admin/env
            cookie=$(curl -sS -o /dev/null -D - --data-urlencode "token=$ADMIN_TOKEN" "$vw/admin" \
              | grep -io '^set-cookie: *VW_ADMIN=[^;]*' | sed 's/^[^:]*: *//') || true
            [ -n "$cookie" ] || { echo "Vaultwarden did not take the admin token; next time"; exit 0; }
            for name in $due; do
              case "$name" in *[!a-z0-9._-]* | [!a-z0-9]*) continue ;; esac
              id=$(echo "$list" | jq -r --arg n "$name" '.members[] | select(.name == $n) | .id')
              code=$(curl -sS -o /tmp/user.json -w '%{http_code}' -H "Cookie: $cookie" "$vw/admin/users/by-mail/$name@${base}")
              if [ "$code" = 200 ]; then
                uuid=$(jq -r .id /tmp/user.json)
                case "$uuid" in "" | *[!0-9a-f-]*) echo "member $id: no account id"; continue ;; esac
                if ! curl -fsS -o /dev/null -X POST -H "Cookie: $cookie" -H 'Content-Type: application/json' -d '{}' \
                  "$vw/admin/users/$uuid/delete"; then
                  echo "member $id: Vaultwarden refused the delete (the last owner of a shared organization?)"
                  continue
                fi
                echo "member $id: their Passwords account is deleted"
              elif [ "$code" != 404 ]; then
                echo "member $id: Vaultwarden answered $code; next time"
                continue
              fi
              curl -fsS -X POST -H 'Content-Type: application/json' \
                -d "$(jq -cn --arg n "$name" '{name: $n, service: "passwords"}')" "$gate/internal/leaving/done" \
                || echo "member $id: the gate did not take the report"
            done
          '';
        };
        systemd.timers.dd-forget-passwords = {
          wantedBy = [ "timers.target" ];
          timerConfig = {
            OnActiveSec = "5min";
            # often: someone who leaves is due at once
            OnUnitActiveSec = "5min";
          };
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
        # laptop's tailnet - except what a Send needs and the page that hands
        # a browser to the app: someone outside opening a link gets the Send
        # page and the calls it makes, and nothing else.
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
              # (app/src/local.rs) and is never served from here. A browser
              # is told to open Passwords in the app; the same page starts a
              # Bitwarden app's sign-in and opens a Send (use-app.js). From
              # outside too: a member's browser off the network reaches this
              # name through the tunnel, and a Send-only page there was a
              # dead end for the bar's Passwords ("Nothing to open"). The
              # page holds no key and its sign-in calls stay inside.
              # the admin page is for dd-forget-passwords on loopback alone
              "^~ /admin".return = "404";
              "/" = {
                extraConfig = ''
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
              # a Send: use-app.js sends a #/send/ link here
              "= /send" = {
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
