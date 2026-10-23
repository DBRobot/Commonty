# Mail to members, for any service on the box. A service knows a member as
# <name>@<domain>, and that is where the mail goes: dd-sendmail hands it to
# the mail Worker (modules/mail.nix), which sends it to the address that
# member gave it (client/mail), so no box ever holds anyone's address.
# A service uses config.dd.mail.sendmail, with the group config.dd.mail.group.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  base = config.dd.domain;
  sendmail = config.dd.mail.sendmail;
in
{
  imports = [ ./mail.nix ];

  options.dd.announce.dir = lib.mkOption {
    type = lib.types.str;
    default = "/etc/dd-announce/release";
    description = "where the release's announcements are (fleet/announcements); a test points it at a directory it writes";
  };

  # `dd policy announce` writes a change into the release, one json each
  # (fleet/announcements, client/cli/src/policy.rs): mailed to each member's
  # <name>@<domain> it names, once each, and refused here too under 30 days'
  # notice. It comes signed with the release; nobody logs in to put it here
  config = {
    environment.etc."dd-announce/release".source = ../../fleet/announcements;
    # What was sent, and to whom, is kept in postgres beside the vault, so
    # it follows the vault's lease: a box that takes it over sends nobody
    # a change twice. A box's own directory would not
    services.postgresql.ensureDatabases = [ "dd_announce" ];
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
      after = [ "postgresql.service" ];
      path = [
        pkgs.jq
        pkgs.coreutils
        pkgs.util-linux
        config.services.postgresql.package
      ];
      serviceConfig = {
        Type = "oneshot";
        PrivateTmp = true;
      };
      # whatever goes wrong is said and tried again, never a failed unit
      script = ''
        set +e
        # ids and names are checked below to be [a-z0-9.-_] before they reach it
        q() { runuser -u postgres -- psql -d dd_announce -v ON_ERROR_STOP=1 -tAqc "$1"; }
        q "CREATE TABLE IF NOT EXISTS sent (id text, name text, PRIMARY KEY (id, name));
           CREATE TABLE IF NOT EXISTS done (id text PRIMARY KEY, how text NOT NULL)" \
          || { echo "its database is not there yet"; exit 0; }
        soonest=$(date -u -d '+30 days' +%F)
        # each in the release, until it is sent (or refused): never twice
        for f in ${config.dd.announce.dir}/*.json; do
          [ -e "$f" ] || continue
          id=$(jq -r .id "$f")
          case "$id" in "" | *[!A-Za-z0-9-]*) echo "$f: no usable id; left"; continue ;; esac
          [ -n "$(q "SELECT 1 FROM done WHERE id = '$id'")" ] && continue
          when=$(jq -r .takes_effect "$f")
          if ! [[ "$when" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || [[ "$when" < "$soonest" ]]; then
            echo "$id: takes effect $when, under 30 days' notice; not sent"
            q "INSERT INTO done VALUES ('$id', 'refused') ON CONFLICT DO NOTHING"
            continue
          fi
          left=0
          for name in $(jq -r '.to[]' "$f"); do
            case "$name" in *[!a-z0-9._-]* | [!a-z0-9]*) continue ;; esac
            [ -n "$(q "SELECT 1 FROM sent WHERE id = '$id' AND name = '$name'")" ] && continue
            if {
              printf 'From: Commonty <noreply@${base}>\nTo: %s@${base}\n' "$name"
              printf 'Subject: %s\nDate: %s\nMIME-Version: 1.0\nContent-Type: text/plain; charset=utf-8\n\n' \
                "$(jq -r .subject "$f" | tr -d '\r\n')" "$(date -R)"
              jq -r .body "$f"
            } | ${sendmail} "$name@${base}"; then
              q "INSERT INTO sent VALUES ('$id', '$name') ON CONFLICT DO NOTHING" \
                || left=$((left + 1))
            else
              left=$((left + 1))
            fi
          done
          if [ "$left" = 0 ]; then
            q "INSERT INTO done VALUES ('$id', 'sent') ON CONFLICT DO NOTHING"
            echo "$id: sent to $(q "SELECT count(*) FROM sent WHERE id = '$id'") member(s)"
          else
            echo "$id: $left not sent yet; again in five minutes"
          fi
        done
      '';
    };
  };
}
