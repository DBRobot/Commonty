# Mail to members, for any service on the box. A service knows a member as
# <name>@<domain>, and that is where the mail goes: dd-sendmail hands it to
# the mail Worker (modules/mail.nix), which sends it to the address that
# member gave it (client/mail), so no box ever holds anyone's address.
# A service uses config.dd.mail.sendmail, with the group config.dd.mail.group.
{
  config,
  pkgs,
  ...
}:
let
  base = config.dd.domain;
  sendmail = config.dd.mail.sendmail;
in
{
  imports = [ ./mail.nix ];

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
        OnActiveSec = "5min";
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
        PrivateTmp = true;
      };
      # whatever goes wrong is said and tried again, never a failed unit
      script = ''
        set +e
        cd /var/lib/dd-announce || exit 0
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
              printf 'From: Commonty <noreply@${base}>\nTo: %s@${base}\n' "$name"
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
