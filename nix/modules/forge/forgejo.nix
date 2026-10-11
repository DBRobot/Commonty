{
  ddScript,
  config,
  options,
  pkgs,
  lib,
  ...
}:
let
  attachable = lib.concatStringsSep "," [
    ".png"
    ".jpg"
    ".jpeg"
    ".gif"
    ".webp"
    ".avif"
    ".pdf"
    ".txt"
    ".log"
    ".md"
    ".csv"
    ".json"
    ".patch"
    ".zip"
    ".gz"
    ".tgz"
    ".mp4"
    ".webm"
    ".mov"
  ];
  # and on a release, the app's downloads (`dd release app` uploads them;
  # commonty.org/download serves them)
  releasable = lib.concatStringsSep "," [
    attachable
    ".deb"
    ".appimage"
    ".apk"
    ".exe"
  ];
  base = config.dd.domain;
  host = "git.${base}";
  # Forgejo believes X-WEBAUTH-USER from whoever reaches it, and this box
  # runs CI jobs and game guests. A socket in a directory only forgejo's
  # group may enter is how "from nginx, or from one of our own units" gets
  # said in a way a job cannot say for itself.
  sock = "/run/forgejo/forgejo.sock";
  cancelPort = 3003;
  cfg = config.dd.forgejo;
  # the forge, as whoever the gate says is looking
  toForge = {
    proxyPass = "http://unix:${sock}:";
    extraConfig = ''
      client_max_body_size 0; # pushes over https, lfs
      # who is this? the verifier says. The header the client sent is
      # replaced either way, so nobody names themselves.
      auth_request /_dd/verify;
      auth_request_set $auth_user $upstream_http_x_auth_request_preferred_username;
      # the demo is nobody here: public repos, no account made for it
      proxy_set_header X-WEBAUTH-USER $forge_user;
      # nobody: still let them in, as nobody. Public repos are public and
      # forgejo shows private ones to no one it does not know.
      error_page 401 = @anonymous;
    '';
    # no cookie when the header is empty (the map below)
    ddCookie = "$forge_cookie";
  };
  admin = lib.replaceStrings [ "'" ] [ "''" ] cfg.admin;
  # What the forge's database holds to, whatever asked for otherwise; put
  # in after every Forgejo start, as live.nix's trigger is, so a table an
  # upgrade rebuilt gets it back.
  rules = pkgs.writeText "dd-forge-rules.sql" ''
    -- No new private repository, and none made private: a repository here
    -- is plaintext on the box, so "private" would be a promise the box
    -- cannot keep. Forgejo has no setting that forbids one (FORCE_PRIVATE
    -- is the opposite), so the database refuses it, through the API, a
    -- push option or a migration alike. One that is private already stays
    -- as it is until its owner makes it public.
    CREATE OR REPLACE FUNCTION dd_no_private_repo() RETURNS trigger LANGUAGE plpgsql AS $f$
    BEGIN
      IF NEW.is_private AND (TG_OP = 'INSERT' OR NOT OLD.is_private) THEN
        RAISE EXCEPTION 'private repositories are not offered here: use an encrypted repository';
      END IF;
      RETURN NEW;
    END $f$;
    DROP TRIGGER IF EXISTS dd_no_private_repo ON repository;
    CREATE TRIGGER dd_no_private_repo BEFORE INSERT OR UPDATE ON repository
      FOR EACH ROW EXECUTE FUNCTION dd_no_private_repo();

    -- A host runner runs a job on the box itself, so it takes the admin's
    -- repositories and no one else's. Whatever token registered it, a
    -- runner for every repository may carry the members' label alone (the
    -- sandboxed runner, members-runner.nix); any other is moved to the
    -- admin's own scope, or with no admin yet to a scope no repository has.
    CREATE OR REPLACE FUNCTION dd_runner_scope() RETURNS trigger LANGUAGE plpgsql AS $f$
    DECLARE
      labels jsonb;
      members_only boolean;
    BEGIN
      IF NEW.owner_id = 0 AND NEW.repo_id = 0 THEN
        BEGIN
          labels := coalesce(nullif(NEW.agent_labels, '''), '[]')::jsonb;
        EXCEPTION WHEN others THEN
          labels := '[]';
        END;
        IF jsonb_typeof(labels) <> 'array' THEN
          labels := '[]';
        END IF;
        -- a label is its name, or its name and how it runs ("members:docker://...")
        SELECT coalesce(bool_and(split_part(l, ':', 1) = 'members'), false) INTO members_only
          FROM jsonb_array_elements_text(labels) AS l;
        IF NOT members_only THEN
          NEW.owner_id := coalesce((SELECT id FROM "user" WHERE lower_name = lower('${admin}')), -1);
        END IF;
      END IF;
      RETURN NEW;
    END $f$;
    DROP TRIGGER IF EXISTS dd_runner_scope ON action_runner;
    CREATE TRIGGER dd_runner_scope BEFORE INSERT OR UPDATE ON action_runner
      FOR EACH ROW EXECUTE FUNCTION dd_runner_scope();
    -- and the runners already registered
    UPDATE action_runner SET owner_id = owner_id WHERE owner_id = 0 AND repo_id = 0;
  '';
in
{
  # the Git pages update as runs and statuses change (live.nix)
  imports = [
    ./live.nix
    ../storage/usage.nix
    ../storage/postgres-backup.nix
  ];

  # The one account that administers the forge. Every other account is made
  # on first visit by the reverse-proxy login below; this one is made ahead
  # of time so that the same login lands on an admin.
  options.dd.forgejo.stateDir = lib.mkOption {
    type = lib.types.str;
    default = "/vault/forgejo";
    description = "repositories, LFS and its config: the mountpoint of <bulk pool>/enc/forgejo";
  };
  options.dd.forgejo.admin = lib.mkOption {
    type = lib.types.str;
    description = "Directory name of the person who administers this forge.";
  };

  config = lib.mkMerge [
    # its own dataset in the box's bulk pool, under the encrypted root
    (lib.optionalAttrs (options.dd ? zfs && options.dd ? box) {
      dd.zfs.datasets."${config.dd.box.bulkPool}/enc/forgejo" = {
        mountpoint = cfg.stateDir;
        recordsize = "128K";
        compression = "zstd";
        "com.sun:auto-snapshot" = "true";
      };
    })
    {
      # reads plaintext: every box it runs on can read these repositories
      # (nix/modules/box/box.nix)
      dd.box.plaintext = [ "forgejo (plain repositories)" ];
      # The forge reads the code; that is what a forge is. So there are no
      # private repositories here (dd-forge-rules refuses one): what must be
      # private goes through the dd remote helper, which stores ciphertext
      # in an ordinary repository here.
      services.forgejo = {
        enable = true;
        package = pkgs.forgejo-lts;
        # on the pool, with snapshots: this is people's work
        stateDir = cfg.stateDir;
        database.type = "postgres"; # the instance ente already runs; socket auth
        lfs.enable = true;
        settings = {
          DEFAULT.APP_NAME = "Commonty";
          server = {
            DOMAIN = host;
            ROOT_URL = "https://${host}/";
            PROTOCOL = "http+unix";
            HTTP_ADDR = sock;
            UNIX_SOCKET_PERMISSION = "660";
            # git over ssh goes through the box's own sshd on 22: forgejo keeps
            # the forgejo user's authorized_keys, sshd does the rest
            START_SSH_SERVER = false;
            SSH_DOMAIN = host;
            SSH_PORT = 22;
            LANDING_PAGE = "explore";
          };
          # Nobody signs up here and nobody has a password here. nginx asks the
          # verifier who this is and says so in a header; forgejo trusts that
          # header from this box alone and makes the account on first sight.
          service = {
            DISABLE_REGISTRATION = true;
            ENABLE_REVERSE_PROXY_AUTHENTICATION = true;
            ENABLE_REVERSE_PROXY_AUTHENTICATION_API = true;
            ENABLE_REVERSE_PROXY_AUTO_REGISTRATION = true;
            ENABLE_REVERSE_PROXY_EMAIL = false;
            ENABLE_PASSWORD_SIGNIN_FORM = false;
            REQUIRE_SIGNIN_VIEW = false; # public repos are public
            DEFAULT_KEEP_EMAIL_PRIVATE = true;
            NO_REPLY_ADDRESS = "noreply.${base}";
          };
          security = {
            REVERSE_PROXY_AUTHENTICATION_USER = "X-WEBAUTH-USER";
            # the socket is the boundary; this stays for forgejo's own sake
            REVERSE_PROXY_TRUSTED_PROXIES = "127.0.0.0/8,::1";
            REVERSE_PROXY_LIMIT = 1;
          };
          session.COOKIE_SECURE = true;
          repository = {
            # public, the only kind there is (dd-forge-rules)
            DEFAULT_PRIVATE = "public";
            DEFAULT_PUSH_CREATE_PRIVATE = false;
            ENABLE_PUSH_CREATE_USER = true;
            # A workflow is code that runs on a box. Every repo may have one:
            # a member's lands on the members' runner
            # (nix/modules/forge/members-runner.nix), sandboxed, and the host
            # runners take the admin's repos alone (dd-forge-rules).
          };
          # What a member may attach to an issue or a release: files that are
          # data, never a page. An .svg or .html served from this name runs as
          # whoever opens it, so neither is on the list. A release takes the
          # app's packages too, and room for them: the AppImage is the
          # biggest, at about 160 MB.
          attachment = {
            ALLOWED_TYPES = attachable;
            MAX_SIZE = 100;
          };
          "repository.release" = {
            ALLOWED_TYPES = releasable;
            FILE_MAX_SIZE = 400;
          };
          mailer.ENABLED = false;
          # a webhook goes out to the world, never to this box, the house or
          # the tailnet
          webhook.ALLOWED_HOST_LIST = "external";
          actions = {
            ENABLED = true; # the runners: nix/modules/forge/forgejo-runner.nix, members-runner.nix
            # `uses: actions/checkout@v4` is written out in full in our
            # workflows; this is where a bare one would be looked up
            DEFAULT_ACTIONS_URL = "github";
          };
          "ui.meta".DESCRIPTION = "git for commonty";
          other.SHOW_FOOTER_VERSION = false;
        };
      };

      # each owner's repositories, counted by the forge itself (storage/usage.nix)
      dd.usage.forgejo = {
        user = "forgejo";
        database = "forgejo";
        query = "select owner_name, coalesce(sum(size + lfs_size), 0) from repository group by owner_name";
        counts = "code";
      };

      # The state dir is a dataset (nix/modules/box/zfs-datasets.nix); do not
      # start before it is mounted, or forgejo would happily initialise itself
      # onto the root disk underneath the mountpoint. The secret bootstrap is
      # the first thing to write there and tmpfiles may have run before the
      # dataset existed (it did, on the activation that created it), so it
      # makes its own directory as root before dropping to forgejo.
      systemd.services.forgejo-secrets = {
        after = [ "zfs-datasets.service" ];
        requires = [ "zfs-datasets.service" ];
        unitConfig.RequiresMountsFor = cfg.stateDir;
        serviceConfig.ExecStartPre = [
          "+${pkgs.coreutils}/bin/install -d -o forgejo -g forgejo -m 0750 ${cfg.stateDir} ${cfg.stateDir}/custom ${cfg.stateDir}/custom/conf"
        ];
      };
      systemd.services.forgejo = {
        after = [ "zfs-datasets.service" ];
        requires = [ "zfs-datasets.service" ];
        unitConfig.RequiresMountsFor = cfg.stateDir;
      };

      # The admin account, made before it is ever logged into. The password is
      # random and never used: the password form is off, the header is the
      # login. Idempotent - "already exists" is the normal case.
      systemd.services.forgejo-admin = {
        description = "Make sure the forge's admin account exists";
        after = [ "forgejo.service" ];
        requires = [ "forgejo.service" ];
        wantedBy = [ "multi-user.target" ];
        path = [ pkgs.forgejo-lts ];
        environment = {
          USER = "forgejo";
          HOME = cfg.stateDir;
          FORGEJO_WORK_DIR = cfg.stateDir;
          FORGEJO_CUSTOM = "${cfg.stateDir}/custom";
        };
        serviceConfig = {
          Type = "oneshot";
          User = "forgejo";
          Group = "forgejo";
        };
        script = ''
          for i in $(seq 1 30); do
            ${pkgs.curl}/bin/curl -fs --unix-socket ${sock} http://forgejo/api/healthz >/dev/null && break
            sleep 2
          done
          if forgejo admin user list --admin | ${pkgs.gnugrep}/bin/grep -qw '${cfg.admin}'; then
            exit 0
          fi
          forgejo admin user create --admin --username '${cfg.admin}' \
            --email '${cfg.admin}@${config.services.forgejo.settings.service.NO_REPLY_ADDRESS}' \
            --random-password --must-change-password=false >/dev/null
          echo "admin ${cfg.admin} created"
        '';
      };

      # main takes merges only, after the forge's own CI has passed on the
      # pull request: nobody pushes it directly, the admin included.
      # Declared here so a rebuilt box gets it back, and idempotent: an
      # existing rule is updated, not duplicated.
      systemd.services.forgejo-protection = {
        description = "Make sure main is protected by the forge's CI";
        after = [ "forgejo-admin.service" ];
        requires = [ "forgejo.service" ];
        wantedBy = [ "multi-user.target" ];
        path = [
          pkgs.curl
          pkgs.jq
        ];
        serviceConfig.Type = "oneshot";
        # the rule main is held to: ci_ok, the job that waits on every other
        # (.forgejo/workflows/entry_point.yml), so the rule names one check
        # that always exists and no job can fail without blocking the merge;
        # and the branch up to date with main, so what merges is the tree ci
        # ran on, not a green branch on an old base
        script = ddScript ./forgejo-setup.sh {
          ADMIN = cfg.admin;
          REPO = config.dd.repo;
          SOCK = sock;
          RULE = builtins.toJSON {
            rule_name = "main";
            branch_name = "main";
            enable_push = false;
            enable_push_whitelist = false;
            push_whitelist_usernames = [ ];
            enable_status_check = true;
            status_check_contexts = [ "entry_point / ci_ok (pull_request)" ];
            block_on_outdated_branch = true;
            required_approvals = 0;
          };
        };
      };

      # the rules above, in the forge's database (as live.nix's trigger)
      systemd.services.dd-forge-rules = {
        description = "No private repositories, and host runners for the admin's alone";
        wantedBy = [ "forgejo.service" ];
        after = [ "forgejo.service" ];
        partOf = [ "forgejo.service" ];
        path = [ config.services.postgresql.package ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          User = "postgres";
        };
        script = ''
          # the first start: Forgejo makes its tables as it comes up
          for _ in $(seq 1 120); do
            [ "$(psql -d ${config.services.forgejo.database.name} -tAc "select to_regclass('public.repository') is not null and to_regclass('public.action_runner') is not null")" = t ] && break
            sleep 1
          done
          psql -d ${config.services.forgejo.database.name} -v ON_ERROR_STOP=1 -q -f ${rules}
        '';
      };

      # a failing ci job stops the rest of its run: the job posts its run id
      # here and this cancels the run as the admin, the one thing the
      # forge's api cannot do. No secret: it cancels only a run the forge
      # already has a failed job in (ci-cancel.py)
      systemd.services.dd-ci-cancel = {
        description = "Cancel a ci run at a failing job's request";
        after = [ "forgejo.service" ];
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          Type = "simple";
          DynamicUser = true;
          SupplementaryGroups = [ "forgejo" ]; # to open forgejo's socket
          ExecStart = "${pkgs.python3}/bin/python3 ${./ci-cancel.py}";
          Restart = "on-failure";
        };
        environment = {
          FORGE = sock;
          ADMIN = cfg.admin;
          REPO = config.dd.repo;
          LISTEN = toString cancelPort;
        };
      };

      # a run killed from outside (the forge or a runner restarting under it)
      # is run again, once: ci-resume.py, every few minutes
      systemd.services.dd-ci-resume = {
        description = "Run again a ci run that a restart killed";
        after = [ "forgejo.service" ];
        # never part of whether a switch came up: 213 rolled back on node1
        # because this ran while the forge was restarting under the switch
        restartIfChanged = false;
        serviceConfig = {
          Type = "oneshot";
          DynamicUser = true;
          SupplementaryGroups = [ "forgejo" ]; # to open forgejo's socket
          StateDirectory = "dd-ci-resume";
          ExecStart = "${pkgs.python3}/bin/python3 ${./ci-resume.py}";
        };
        environment = {
          FORGE = sock;
          ADMIN = cfg.admin;
          REPO = config.dd.repo;
          STATE = "/var/lib/dd-ci-resume";
        };
      };
      systemd.timers.dd-ci-resume = {
        wantedBy = [ "timers.target" ];
        # five minutes after the timer starts, boot or switch: never inside one
        timerConfig = {
          OnActiveSec = "5min";
          OnUnitActiveSec = "3min";
        };
      };

      # Forgejo signs in whoever its own session cookie names when the header
      # is empty, so with an empty header it gets no cookie at all: a session
      # the gate ended, or the demo, is nobody here. The Cookie line is set
      # through ddCookie (nix/modules/gate/strip-cookie.nix), never a second
      # proxy_set_header: nginx would send both lines.
      services.nginx.appendHttpConfig = ''
        map $auth_user $forge_user {
          default $auth_user;
          demo "";
        }
        map $forge_user $forge_cookie {
          "" "";
          default $dd_cookie_stripped;
        }
      '';
      # the socket's directory: forgejo makes it, its group may enter it, and
      # the runner's user - which is what a CI job runs as - is not in it
      systemd.services.forgejo.serviceConfig.RuntimeDirectoryMode = lib.mkForce "0750";
      # the whole attribute, not just its value: naming users.users.nginx
      # where nginx does not run leaves a user with no group and no kind
      users.users = lib.mkIf config.services.nginx.enable {
        nginx.extraGroups = [ "forgejo" ];
      };
      services.nginx.virtualHosts.${host} = {
        useACMEHost = base;
        forceSSL = true;
        # The pages are ours (servers/forge/web): every address that is not the
        # forge's own below gets the one page, which draws itself from the
        # forge's API.
        locations."/" = {
          proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}";
          extraConfig = ''
            rewrite ^ /_dd/git break;
            proxy_set_header X-Original-URI $request_uri;
          '';
        };
        # the forge's own: its API, git over https, raw files and archives,
        # a job's log, avatars, and its account pages. The Git pages keep the
        # same list (FORGE in servers/forge/web/git-core.js): change both.
        locations."~ ^/(api|assets|avatars|repo-avatars|attachments|user|login|-|\\.well-known)(/|$)" =
          toForge;
        locations."~ ^/[^/]+/[^/]+\\.git(/|$)" = toForge;
        locations."~ ^/[^/]+/[^/]+/(info/refs|info/lfs|git-upload-pack|git-receive-pack)" = toForge;
        locations."~ ^/[^/]+/[^/]+/(raw|archive|media|releases/download|attachments)/" = toForge;
        locations."~ ^/[^/]+/[^/]+/actions/runs/[0-9]+/jobs/[0-9]+/attempt/[0-9]+/logs$" = toForge;
        # which repository the boxes are built from: its pages show what they run
        locations."= /fleet-repo.json".extraConfig = ''
          default_type application/json;
          return 200 '${builtins.toJSON { repo = config.dd.repo; }}';
        '';
        # the ci cancel route: no sign-in and no secret, it only stops a run
        # that has already failed (ci-cancel.py)
        locations."/_dd/ci/" = {
          proxyPass = "http://127.0.0.1:${toString cancelPort}";
        };
        locations."@anonymous" = {
          proxyPass = "http://unix:${sock}:";
          extraConfig = ''
            client_max_body_size 0;
            proxy_set_header X-WEBAUTH-USER "";
          '';
          # and not whoever forgejo's own cookie remembers: a session the
          # gate has ended is ended here too
          ddCookie = "";
        };
        # forgejo's own sign-in page is a password form that is switched off;
        # the box's passkey page is the sign-in
        locations."= /user/login".extraConfig = ''
          return 302 /_dd/login?rd=$arg_redirect_to;
        '';
      };
    }
  ];
}
