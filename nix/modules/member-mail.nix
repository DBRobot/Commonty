# Mail to members, for any service on the box. A service knows a member as
# <name>@<domain>, and that is where the mail goes: Cloudflare forwards it to
# the address the member gave (the mail Worker, client/mail), so no box ever
# holds anyone's address.
# A service uses it with
#   SENDMAIL_COMMAND (or its own setting) = config.dd.memberMail.sendmail;
#   serviceConfig.LoadCredential = config.dd.memberMail.credentials;
{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.memberMail;
  base = config.dd.domain;

  # the same relay as the rest of the box (modules/mail.nix); the password is
  # read from the unit's credentials as each mail is sent
  msmtprc = pkgs.writeText "dd-member-msmtprc" ''
    defaults
    auth on
    tls on
    tls_starttls on
    tls_trust_file /etc/ssl/certs/ca-certificates.crt

    account default
    host smtp.gmail.com
    port 587
    from distributed.datacenter@gmail.com
    user distributed.datacenter@gmail.com
    passwordeval cat "$CREDENTIALS_DIRECTORY/smtp"
  '';

  sendmail = pkgs.writeShellScript "dd-member-sendmail" ''
    set -u
    rcpts=()
    skip=
    for a in "$@"; do
      # -f <sender> is the envelope's from, not someone to write to
      [ -n "$skip" ] && { skip=; continue; }
      case "$a" in
        -f) skip=1 ;;
        -*) ;;
        *@*) rcpts+=("$a") ;;
      esac
    done
    [ ''${#rcpts[@]} -gt 0 ] || { cat >/dev/null; exit 0; }
    exec ${pkgs.msmtp}/bin/msmtp -C ${msmtprc} -i -- "''${rcpts[@]}"
  '';
in
{
  options.dd.memberMail = {
    smtpPasswordFile = lib.mkOption {
      type = lib.types.path;
      description = "the relay account's password (sops: ente-smtp-password)";
    };
    sendmail = lib.mkOption {
      type = lib.types.path;
      readOnly = true;
      default = sendmail;
      description = "a sendmail that writes to members by their <name>@<domain>";
    };
    credentials = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      readOnly = true;
      default = [ "smtp:${cfg.smtpPasswordFile}" ];
      description = "the LoadCredential lines a unit using sendmail needs";
    };
  };
}
