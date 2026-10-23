# The runners' registration tokens, made by the box that runs the forge and
# sealed to each runner box's own secrets key (its TPM): no person mints
# them, and no box holds another's in its secrets file. The forge's own
# tool makes them (the host runners' for the fleet repository's owner, the
# members' runner's for every repository), each is sealed with age to each
# runner box, and the sealed files are served at
# git.<domain>/_dd/runner-keys/<box>.<host|members>.age, which only that
# box can open (runner-start.sh fetches its own once the forge answers).
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.forgejo;
  out = "/var/lib/dd-runner-keys";
  sock = "/run/forgejo/forgejo.sock";
in
{
  options.dd.forgejo.runnerBoxes = lib.mkOption {
    type = lib.types.attrsOf lib.types.str;
    default = { };
    description = "each box that runs CI -> its secrets key's recipient (fleet/boxes/<box>/hardware.json secretsKey): whom its registration tokens are sealed to";
  };

  # always there, as a writer's units must be (box/placed.nix): with no
  # runner box it seals nothing
  config = {
    systemd.tmpfiles.rules = [ "d ${out} 0755 forgejo forgejo -" ];
    systemd.services.dd-runner-keys = {
      description = "Make the CI runners' registration tokens, each sealed to its runner box";
      after = [ "forgejo-admin.service" ];
      # a timer's, never a switch's
      restartIfChanged = false;
      path = [
        pkgs.forgejo-lts
        pkgs.age
        pkgs.age-plugin-tpm
        pkgs.curl
        pkgs.coreutils
      ];
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
        UMask = "0022";
      };
      # the forge not up yet is next time, never a failure; a token is never
      # in a command's arguments
      script =
        if cfg.runnerBoxes == { } then
          "echo 'no box runs CI: no token to make'"
        else
          ''
            set -uo pipefail
            curl -fs --unix-socket ${sock} http://forgejo/api/healthz >/dev/null || { echo "the forge is not up yet"; exit 0; }
            host=$(forgejo actions generate-runner-token --scope ${lib.escapeShellArg cfg.hostRunnerOwner}) || exit 0
            members=$(forgejo actions generate-runner-token) || exit 0
            seal() { # <box> <recipient> <kind> <token>
              printf 'TOKEN=%s\n' "$4" | age -r "$2" -o "${out}/$1.$3.age.new" \
                && chmod 0444 "${out}/$1.$3.age.new" \
                && mv "${out}/$1.$3.age.new" "${out}/$1.$3.age" \
                || echo "$1: could not seal its $3 token"
            }
            ${lib.concatStrings (
              lib.mapAttrsToList (b: r: ''
                seal ${lib.escapeShellArg b} ${lib.escapeShellArg r} host "$host"
                seal ${lib.escapeShellArg b} ${lib.escapeShellArg r} members "$members"
              '') cfg.runnerBoxes
            )}
            exit 0
          '';
    };
    systemd.timers.dd-runner-keys = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "2min";
        OnUnitActiveSec = "1h";
      };
    };
    # public: what is there only its own box can open
    services.nginx.virtualHosts."git.${config.dd.domain}".locations."^~ /_dd/runner-keys/" = {
      alias = "${out}/";
      ddCookie = "";
    };
  };
}
