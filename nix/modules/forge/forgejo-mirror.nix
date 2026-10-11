{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.forgejo;
  # Over forgejo's socket the reverse-proxy header is taken at face value,
  # so the admin's name is the whole credential here. What makes that safe
  # is who can open the socket: this unit runs as root, a CI job does not.
  # No forgejo token to mint or keep.
  api = "http://forgejo/api/v1";
  sock = "/run/forgejo/forgejo.sock";
in
{
  # Repositories mirrored out to GitHub after every push, so the public
  # copy there stays current. The forge is the source; nothing depends on
  # the copy (docs/dev/mirror.md).
  options.dd.forgejo.mirrors = lib.mkOption {
    type = lib.types.listOf (
      lib.types.submodule {
        options = {
          repo = lib.mkOption {
            type = lib.types.str;
            example = "david/Home-Server";
          };
          to = lib.mkOption {
            type = lib.types.str;
            example = "https://github.com/DBRobot/Home-Server.git";
          };
          user = lib.mkOption {
            type = lib.types.str;
            description = "GitHub login the token belongs to.";
          };
        };
      }
    );
    default = [ ];
  };

  config = lib.mkIf (cfg.mirrors != [ ]) {
    # a fine-grained GitHub token with contents read/write on the mirrored
    # repositories; forgejo keeps its own encrypted copy once told
    sops.secrets.github-mirror-token = { };

    systemd.services.forgejo-mirrors = {
      description = "Make sure every declared push mirror exists, with the token in sops";
      after = [ "forgejo-admin.service" ];
      requires = [ "forgejo.service" ];
      # a timer's, never a switch's: GitHub refusing a sync (the account is
      # suspended) rolled release 232 back on node1
      restartIfChanged = false;
      path = [
        pkgs.curl
        pkgs.jq
      ];
      serviceConfig = {
        Type = "oneshot";
        LoadCredential = "token:${config.sops.secrets.github-mirror-token.path}";
        # what each mirror was last given, as a hash: forgejo keeps the
        # token encrypted and its API cannot say which one it holds
        StateDirectory = "forgejo-mirrors";
      };
      script = ''
        set -euo pipefail
        # a failed unit counts against the next release's probe: say it, try next time
        trap 'echo "mirror check failed at line $LINENO; next time"; exit 0' ERR
        token=$(cat "$CREDENTIALS_DIRECTORY/token")
        # the forge not answering yet (restarting under a switch) is next time
        as_admin_ok() { curl -fsS --unix-socket ${sock} -o /dev/null ${api}/version; }
        as_admin_ok || { echo "forge not answering; next time"; exit 0; }
        as_admin() { curl -fsS --unix-socket ${sock} -H 'X-WEBAUTH-USER: ${cfg.admin}' "$@"; }
        ${lib.concatMapStringsSep "\n" (m: ''
          # the API has no way to change a mirror's token: one given another
          # token (a rotation in sops) is removed and made again
          given=$(printf '%s\n' "${m.repo}" "${m.to}" "${m.user}" "$token" | sha256sum | cut -d' ' -f1)
          kept="$STATE_DIRECTORY/$(printf '%s %s' "${m.repo}" "${m.to}" | sha256sum | cut -d' ' -f1)"
          names=$(as_admin ${api}/repos/${m.repo}/push_mirrors | jq -r '.[] | select(.remote_address == "${m.to}") | .remote_name')
          if [ -n "$names" ] && [ "$(cat "$kept" 2>/dev/null)" = "$given" ]; then
            echo "${m.repo} -> ${m.to}: present"
          else
            for name in $names; do
              as_admin -X DELETE ${api}/repos/${m.repo}/push_mirrors/"$name" >/dev/null
            done
            jq -n --arg addr "${m.to}" --arg user "${m.user}" --arg pw "$token" \
              '{remote_address: $addr, remote_username: $user, remote_password: $pw, interval: "8h0m0s", sync_on_commit: true}' \
              | as_admin -X POST -H 'content-type: application/json' -d @- ${api}/repos/${m.repo}/push_mirrors >/dev/null
            echo "$given" >"$kept"
            if as_admin -X POST ${api}/repos/${m.repo}/push_mirrors-sync >/dev/null; then
              echo "${m.repo} -> ${m.to}: ''${names:+token changed, }added and first sync requested"
            else
              echo "${m.repo} -> ${m.to}: ''${names:+token changed, }added; the first sync was refused, next push tries again"
            fi
          fi
        '') cfg.mirrors}
      '';
    };
    systemd.timers.forgejo-mirrors = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "5min";
        OnUnitActiveSec = "6h";
      };
    };
  };
}
