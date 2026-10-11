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

  # `dd policy announce` leaves a change in the outbox, one json each
  # (client/cli/src/policy.rs): mailed to each member's <name>@<domain>,
  # once each, and refused here too under 30 days' notice
  config = {
    systemd.tmpfiles.rules = [
      "d /var/lib/dd-announce 0700 root root -"
      "d /var/lib/dd-announce/outbox 0700 root root -"
      "d /var/lib/dd-announce/sent 0700 root root -"
    ];
    systemd.timers.dd-announce = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1min";
        OnUnitActiveSec = "5min";
      };
    };
    systemd.services.dd-announce = {
      description = "Mail a policy change to every member";
      restartIfChanged = false;
      path = [
        pkgs.jq
        pkgs.coreutils
        pkgs.gnugrep
      ];
      serviceConfig = {
        Type = "oneshot";
        LoadCredential = cfg.credentials;
        PrivateTmp = true;
      };
      script = ''
        cd /var/lib/dd-announce
        soonest=$(date -u -d '+30 days' +%F)
        for f in outbox/*.json; do
          [ -e "$f" ] || continue
          id=$(jq -r .id "$f")
          case "$id" in "" | *[!A-Za-z0-9-]*) echo "$f: no usable id; set aside"; mv "$f" "sent/bad-$(date +%s).json"; continue ;; esac
          when=$(jq -r .takes_effect "$f")
          if ! [[ "$when" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || [[ "$when" < "$soonest" ]]; then
            echo "$id: takes effect $when, under 30 days' notice; not sent"
            mv "$f" "sent/$id.refused.json"
            continue
          fi
          touch "sent/$id.names"
          left=0
          for name in $(jq -r '.to[]' "$f"); do
            case "$name" in *[!a-z0-9._-]* | [!a-z0-9]*) continue ;; esac
            grep -qxF "$name" "sent/$id.names" && continue
            if {
              printf 'From: Commonty <distributed.datacenter@gmail.com>\nTo: %s@${base}\n' "$name"
              printf 'Subject: %s\nDate: %s\nMIME-Version: 1.0\nContent-Type: text/plain; charset=utf-8\n\n' \
                "$(jq -r .subject "$f" | tr -d '\r\n')" "$(date -R)"
              jq -r .body "$f"
            } | ${sendmail} "$name@${base}"; then
              echo "$name" >> "sent/$id.names"
            else
              left=$((left + 1))
            fi
          done
          if [ "$left" = 0 ]; then
            mv "$f" "sent/$id.json"
            echo "$id: sent to $(wc -l < "sent/$id.names") member(s)"
          else
            echo "$id: $left not sent yet; again in five minutes"
          fi
        done
      '';
    };
  };
}
