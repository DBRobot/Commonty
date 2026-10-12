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
  # The GitHub copy is pushed by the machine that publishes the release,
  # over ssh with the owner's own key (dd release publish; docs/dev/mirror.md),
  # never by the forge: a push mirror here holds a GitHub token in the
  # forge's database, and no box holds a credential of the owner's. What is
  # left here takes away any push mirror to those copies, so the token the
  # forge was once given goes with it.
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
            default = "";
            description = "no longer used: the copy is pushed over ssh as its owner";
          };
        };
      }
    );
    default = [ ];
    description = "the fleet's copies elsewhere (fleet/policy.json forgeMirrors), which the forge must not push itself";
  };

  config = lib.mkIf (cfg.mirrors != [ ]) {
    systemd.services.forgejo-mirrors = {
      description = "Take away any push mirror to the fleet's copies, and the token it holds";
      after = [ "forgejo-admin.service" ];
      requires = [ "forgejo.service" ];
      # a timer's, never a switch's
      restartIfChanged = false;
      path = [
        pkgs.curl
        pkgs.jq
      ];
      serviceConfig.Type = "oneshot";
      script = ''
        set -euo pipefail
        # a failed unit counts against the next release's probe: say it, try next time
        trap 'echo "mirror check failed at line $LINENO; next time"; exit 0' ERR
        # the forge not answering yet (restarting under a switch) is next time
        curl -fsS --unix-socket ${sock} -o /dev/null ${api}/version || { echo "forge not answering; next time"; exit 0; }
        as_admin() { curl -fsS --unix-socket ${sock} -H 'X-WEBAUTH-USER: ${cfg.admin}' "$@"; }
        ${lib.concatMapStringsSep "\n" (m: ''
          for name in $(as_admin ${api}/repos/${m.repo}/push_mirrors | jq -r '.[] | select(.remote_address == "${m.to}") | .remote_name'); do
            as_admin -X DELETE ${api}/repos/${m.repo}/push_mirrors/"$name" >/dev/null
            echo "${m.repo} -> ${m.to}: push mirror $name removed, and the token it held"
          done
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
