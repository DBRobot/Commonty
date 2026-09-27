{
  ddScript,
  vmTests,
  boxNames,
  config,
  pkgs,
  lib,
  ...
}:
let
  base = config.dd.domain;
  host = "git.${base}";
  port = 3001; # 3000 is grafana; kept for the runner's own url only
  # Forgejo believes X-WEBAUTH-USER from whoever reaches it, and this box
  # runs CI jobs and game guests. A socket in a directory only forgejo's
  # group may enter is how "from nginx, or from one of our own units" gets
  # said in a way a job cannot say for itself.
  sock = "/run/forgejo/forgejo.sock";
  cancelPort = 3003;
  cfg = config.dd.forgejo;
in
{
  # The one account that administers the forge. Every other account is made
  # on first visit by the reverse-proxy login below; this one is made ahead
  # of time so that the same login lands on an admin.
  options.dd.forgejo.admin = lib.mkOption {
    type = lib.types.str;
    description = "Directory name of the person who administers this forge.";
  };
  options.dd.forgejo.ciSecretFile = lib.mkOption {
    type = lib.types.path;
    description = "the secret a ci job shows to stop its run (ci-cancel.py); the same value is the repo's DD_CI actions secret";
  };

  config = {
    # reads plaintext: only on a box whose owner is trusted with it (modules/box.nix)
    dd.box.plaintext = [ "forgejo (plain repositories)" ];
    # Private repositories here are private from the world, not from whoever
    # runs this box. The forge reads the code; that is what a forge is. What
    # must be private from the box too goes through the dd remote helper,
    # which stores ciphertext in an ordinary repository here.
    services.forgejo = {
      enable = true;
      package = pkgs.forgejo-lts;
      # on the pool, with snapshots: this is people's work
      stateDir = "/vault/forgejo";
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
          DEFAULT_PRIVATE = "public";
          DEFAULT_PUSH_CREATE_PRIVATE = false;
          ENABLE_PUSH_CREATE_USER = true;
          # A workflow is code that runs on a box. Every repo may have one:
          # a member's lands on the members' runner (modules/members-runner.nix),
          # sandboxed, and the host runners take the owner's repos alone.
        };
        mailer.ENABLED = false;
        actions = {
          ENABLED = true; # the runner is modules/forgejo-runner.nix
          # `uses: actions/checkout@v4` is written out in full in our
          # workflows; this is where a bare one would be looked up
          DEFAULT_ACTIONS_URL = "github";
        };
        "ui.meta".DESCRIPTION = "git for commonty";
        other.SHOW_FOOTER_VERSION = false;
      };
    };

    # The state dir is a dataset (modules/zfs-datasets.nix); do not start
    # before it is mounted, or forgejo would happily initialise itself onto
    # the root disk underneath the mountpoint. The secret bootstrap is the
    # first thing to write there and tmpfiles may have run before the dataset
    # existed (it did, on the activation that created it), so it makes its
    # own directory as root before dropping to forgejo.
    systemd.services.forgejo-secrets = {
      after = [ "zfs-datasets.service" ];
      requires = [ "zfs-datasets.service" ];
      unitConfig.RequiresMountsFor = "/vault/forgejo";
      serviceConfig.ExecStartPre = [
        "+${pkgs.coreutils}/bin/install -d -o forgejo -g forgejo -m 0750 /vault/forgejo /vault/forgejo/custom /vault/forgejo/custom/conf"
      ];
    };
    systemd.services.forgejo = {
      after = [ "zfs-datasets.service" ];
      requires = [ "zfs-datasets.service" ];
      unitConfig.RequiresMountsFor = "/vault/forgejo";
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
        HOME = "/vault/forgejo";
        FORGEJO_WORK_DIR = "/vault/forgejo";
        FORGEJO_CUSTOM = "/vault/forgejo/custom";
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
    # pull request. Declared here so a rebuilt box gets it back, and
    # idempotent: an existing rule is updated, not duplicated. The admin
    # may still push main directly while GitHub is where merges happen;
    # that allowance goes when the forge is the only source.
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
      # the rule main is held to: every ci job, from the same lists the
      # flake runs (vmTests, the boxes), so a new test is required the
      # moment it exists
      script = ddScript ./forgejo-setup.sh {
        ADMIN = cfg.admin;
        REPO = config.dd.repo;
        SOCK = sock;
        CI_SECRET_FILE = cfg.ciSecretFile;
        RULE = builtins.toJSON {
          rule_name = "main";
          branch_name = "main";
          enable_push = true;
          enable_push_whitelist = true;
          push_whitelist_usernames = [ cfg.admin ];
          enable_status_check = true;
          status_check_contexts = map (j: "entry_point / ${j} (pull_request)") (
            [
              "flake_check"
              "build_client"
            ]
            ++ map (b: "build_host (${b})") boxNames
            ++ [
              "lint"
              "test"
            ]
            ++ map (t: "vm_tests (${t})") vmTests
          );
          block_on_outdated_branch = false;
          required_approvals = 0;
        };
      };
    };

    # a failing ci job stops the rest of its run: the job posts here with
    # the ci secret and this cancels the run as the admin, the one thing
    # the forge's api cannot do
    systemd.services.dd-ci-cancel = {
      description = "Cancel a ci run at a failing job's request";
      after = [ "forgejo.service" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "simple";
        DynamicUser = true;
        SupplementaryGroups = [ "forgejo" ]; # to open forgejo's socket
        LoadCredential = [ "secret:${cfg.ciSecretFile}" ];
        ExecStart = "${pkgs.python3}/bin/python3 ${./ci-cancel.py}";
        Restart = "on-failure";
      };
      environment = {
        FORGE = sock;
        ADMIN = cfg.admin;
        REPO = config.dd.repo;
        SECRET_FILE = "%d/secret";
        LISTEN = toString cancelPort;
      };
    };

    services.nginx.appendHttpConfig = ''
      map $auth_user $forge_user {
        default $auth_user;
        demo "";
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
      locations."/" = {
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
      };
      # the ci cancel route: the secret is the credential, not a session
      locations."/_dd/ci/" = {
        proxyPass = "http://127.0.0.1:${toString cancelPort}";
      };
      locations."@anonymous" = {
        proxyPass = "http://unix:${sock}:";
        extraConfig = ''
          client_max_body_size 0;
          proxy_set_header X-WEBAUTH-USER "";
        '';
      };
      # forgejo's own sign-in page is a password form that is switched off;
      # the box's passkey page is the sign-in
      locations."= /user/login".extraConfig = ''
        return 302 /_dd/login?rd=$arg_redirect_to;
      '';
    };
  };
}
