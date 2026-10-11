# The services the control plane switches (needs placed = true): their
# units on this box start only once /run/dd-placed/<service> exists, and
# dd-control puts it there and takes it away by the plan
# (servers/system/control/cmd/dd-control/placed.go). /run is empty at boot,
# so a placed service runs nowhere until the agreed state puts it somewhere.
# What this box carries - the placed services its roles bring - is
# /etc/dd-placed/carries.json, and its heartbeat, so the plan only puts a
# service where it can run.
#
# A writer (needs lease) is also stopped by a timer of its own when its
# lease may have lapsed, so a dd-control that crashed or hangs does not
# leave it writing beside the next holder.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  needs = import ../../lib/needs.nix { inherit lib; };
  dir = "/run/dd-placed";
  # a box carries a placed service when one of its roles brings it
  # (fleet/roles.json); its units are the service and the ones its needs
  # name. Not read from config.systemd, which these conditions are part of
  roles = lib.filterAttrs (_: v: builtins.isAttrs v) (lib.importJSON ../../../fleet/roles.json);
  brought = lib.concatMap (r: roles.${r}.services or [ ]) config.dd.control.roles;
  carries = lib.genAttrs (lib.filter (
    s: (needs.${s}.placed || needs.${s}.lease) && builtins.elem s brought
  ) (builtins.attrNames needs)) (s: needs.${s}.units);
  when = svc: { unitConfig.ConditionPathExists = "${dir}/${svc}"; };
in
{
  config = lib.mkIf config.dd.control.enable {
    # a name that is no unit would be made one by its condition, with
    # nothing to run, and fail the start of all the rest
    assertions = lib.mapAttrsToList (s: units: {
      assertion =
        units != [ ]
        && lib.all (
          u:
          !(lib.hasInfix "*" u || lib.hasSuffix "@" u)
          && config.systemd.services.${u}.serviceConfig ? ExecStart
        ) units;
      message = "placed service ${s}: its needs' units must name every unit it is, one by one (no * or templates), and each must be a service this box defines";
    }) carries;
    environment.etc."dd-placed/carries.json".text = builtins.toJSON (
      lib.mapAttrs (_: map (u: "${u}.service")) carries
    );
    systemd.tmpfiles.rules = [ "d ${dir} 0755 root root -" ];
    systemd.services = lib.mkMerge (
      [
        {
          dd-placed-fence = {
            description = "Stop a writer whose lease may have lapsed";
            restartIfChanged = false;
            path = [
              pkgs.jq
              pkgs.coreutils
              pkgs.systemd
            ];
            serviceConfig.Type = "oneshot";
            script = builtins.readFile ./placed-fence.sh;
          };
        }
      ]
      ++ lib.mapAttrsToList (s: units: lib.genAttrs units (_: when s)) carries
    );
    # a timer of a placed service fires everywhere; the service it starts
    # does not, without the flag
    systemd.timers.dd-placed-fence = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "2s";
        OnUnitActiveSec = "2s";
        AccuracySec = "500ms";
      };
    };
  };
}
