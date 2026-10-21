# Members' locked mail on this box (servers/gate/src/mail.rs, docs/dev/mail.md):
# the mail bucket, beside the libraries and under the same gate key, and the
# gate taking from the mail Worker what it has locked to members' keys. The
# gate signs those requests through a socket of root's that signs nothing
# else: no key of the box's is the gate's.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.mailInbox;
  sock = "/run/dd-mail-hold/sign.sock";
  sign = pkgs.writeShellScript "dd-mail-hold-sign" (
    builtins.replaceStrings [ "@boxSign@" ] [ "${../gate/box-sign.sh}" ] (
      builtins.readFile ./mail-hold-sign.sh
    )
  );
in
{
  options.dd.mailInbox = {
    enable = lib.mkEnableOption "members' locked mail on this box";
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = config.dd.libraries.enable;
        message = "the mail bucket lives beside the libraries, under their key";
      }
    ];
    dd.garage.buckets.mail = {
      key = {
        name = "dd-library";
        envPrefix = "LIBRARY";
      };
      allow = [
        "read"
        "write"
      ];
      # no CORS: only the gate reaches it
    };
    # a member's app reaches its mail at files.<domain>, as it does its
    # libraries: bodies up to the gate's own cap, and the pipe a WebSocket
    services.nginx.virtualHosts."files.${config.dd.domain}".locations = {
      # the page itself (the tile's address), as the gate's other pages: a
      # prefix location ending in / would answer this name with a 301 to
      # /_dd/mail/, which nothing serves
      "= /_dd/mail" = {
        proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/mail";
        extraConfig = ''
          proxy_set_header X-Original-URI $request_uri;
          limit_req zone=dd_user burst=300 nodelay;
          limit_req zone=dd_visitor burst=500 nodelay;
          limit_req_status 429;
        '';
      };
      "/_dd/mail/" = {
        proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/mail/";
        extraConfig = ''
          # signed in, asked before a byte of the body is taken: nobody
          # else has nginx hold an upload
          auth_request /_dd/verify;
          # the gate's own cap on one locked object (mail.rs MAX), streamed
          client_max_body_size 33m;
          proxy_request_buffering off;
          proxy_set_header X-Original-URI $request_uri;
          limit_req zone=dd_user burst=100 nodelay;
          limit_req zone=dd_visitor burst=100 nodelay;
          limit_req_status 429;
        '';
      };
      "= /_dd/mail/pipe" = {
        proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/mail/pipe";
        proxyWebsockets = true;
        extraConfig = ''
          auth_request /_dd/verify;
          proxy_set_header X-Original-URI $request_uri;
          proxy_read_timeout 660s;
          proxy_send_timeout 660s;
          limit_req zone=dd_user burst=30 nodelay;
          limit_req_status 429;
        '';
      };
    };
    # the tile: the app serves the page, which holds the key (mail.js)
    dd.home.services = [
      {
        name = "Mail";
        blurb = "Your email, locked as it arrives to a key only your devices hold.";
        url = "https://files.${config.dd.domain}/_dd/mail";
        icon = "mail";
        color = "#1d5c42";
        rank = 25;
      }
    ];
    users.groups.dd-mail-hold = { };
    systemd.services.dd-verify = {
      environment = {
        VERIFY_MAIL_WORKER = "https://mail.${config.dd.domain}";
        VERIFY_MAIL_BOX = config.networking.hostName;
        VERIFY_MAIL_SIGN = sock;
        VERIFY_MAIL_BUCKET = "mail";
      };
      serviceConfig.SupplementaryGroups = [ "dd-mail-hold" ];
    };
    systemd.sockets.dd-mail-hold-sign = {
      description = "Signs the gate's taking of held mail, and nothing else";
      wantedBy = [ "sockets.target" ];
      listenStreams = [ sock ];
      socketConfig = {
        Accept = true;
        SocketMode = "0660";
        SocketGroup = "dd-mail-hold";
        MaxConnections = 8;
      };
    };
    # one per signature; a refusal is an empty answer, never a failed unit
    systemd.services."dd-mail-hold-sign@" = {
      description = "Sign one hold request with the TPM";
      path = [
        pkgs.tpm2-tools
        pkgs.openssl
        pkgs.coreutils
        pkgs.gnused
      ];
      environment = {
        TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
        BOX = config.networking.hostName;
        KEYS = "/boot/dd/unlock";
      };
      serviceConfig = {
        ExecStart = sign;
        StandardInput = "socket";
        StandardOutput = "socket";
        StandardError = "journal";
        TimeoutSec = 30;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        NoNewPrivileges = true;
      };
    };
  };
}
