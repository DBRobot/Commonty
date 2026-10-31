# Mail from this box: alerts to the owner and mail to members, handed to the
# mail Worker at mail.<domain> (client/mail/worker/send.js) and sent on by
# Cloudflare. No mail account and no password on the box: each message is
# signed by this box's TPM key (the unlock's, box/disk-unlock.nix), and the
# Worker sends only to "alerts" (the owner's address) or a member's own
# <name>@<domain>.
#
# A service uses it with
#   its sendmail = config.dd.mail.sendmail;   (bin/dd-sendmail)
#   serviceConfig.SupplementaryGroups = [ config.dd.mail.group ];
# Root needs no group. The signing is root's alone, behind a socket.
{
  config,
  lib,
  pkgs,
  ddScript,
  ...
}:
let
  sock = "/run/dd-mail/send.sock";
  send = pkgs.writeShellScript "dd-mail-send" (
    ddScript ./mail-send.sh {
      BOX = config.networking.hostName;
      URL = "https://mail.${config.dd.domain}";
      DOMAIN = config.dd.domain;
      KEYS = "/boot/dd/unlock";
      # the Worker's own cap (send.js)
      MAX = 256 * 1024;
    }
  );
  sendmail = pkgs.writeShellApplication {
    name = "dd-sendmail";
    runtimeInputs = [
      pkgs.socat
      pkgs.coreutils
    ];
    text = ddScript ./sendmail.sh { SOCK = sock; };
  };
in
{
  options.dd.mail = {
    sendmail = lib.mkOption {
      type = lib.types.path;
      readOnly = true;
      default = "${sendmail}/bin/dd-sendmail";
      description = "a sendmail that writes to \"alerts\" or a member's <name>@<domain>, through the mail Worker";
    };
    group = lib.mkOption {
      type = lib.types.str;
      readOnly = true;
      default = "dd-mail";
      description = "the group a non-root unit needs to use sendmail";
    };
  };

  config = {
    users.groups.dd-mail = { };
    environment.systemPackages = [ sendmail ];
    systemd.sockets.dd-mail = {
      description = "Mail from this box, to the mail Worker";
      wantedBy = [ "sockets.target" ];
      listenStreams = [ sock ];
      socketConfig = {
        Accept = true;
        SocketMode = "0660";
        SocketGroup = "dd-mail";
        MaxConnections = 8;
      };
    };
    # one per message; a failure is said to the sender, never a failed unit
    systemd.services."dd-mail@" = {
      description = "Sign one mail with the TPM and hand it to the mail Worker";
      path = [
        pkgs.tpm2-tools
        pkgs.util-linux # flock: one user of the TPM at a time
        (import ../lib/tpm-policy.nix pkgs)
        pkgs.curl
        pkgs.jq
        pkgs.coreutils
      ];
      environment.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
      serviceConfig = {
        ExecStart = send;
        StandardInput = "socket";
        StandardOutput = "socket";
        StandardError = "journal";
        TimeoutSec = 60;
        # root for the TPM and the key files on /boot; nothing is written
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        NoNewPrivileges = true;
      };
    };
  };
}
