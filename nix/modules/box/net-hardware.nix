# The box's network hardware, as its report found it (box/report/nics.sh),
# made into its config by nix/lib/net-hardware.nix: the cable's drivers in
# the initrd, the Wi-Fi fallback only where there is an adapter for it, and
# a warning for an adapter known to fail - at every build of the box's
# config, and in its journal at every boot (dd-net-check).
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.box.links;
  derived = import ../../lib/net-hardware.nix {
    hw = cfg.report;
    inherit (cfg) houseBoxes;
  };
in
{
  options.dd.box.links = {
    report = lib.mkOption {
      type = lib.types.nullOr lib.types.attrs;
      default = null;
      description = "the box's own report (fleet/boxes/<box>/hardware.json); null: a box without one, which gets nothing from here";
    };
    houseBoxes = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      description = "the other boxes in its house, by name, at their house addresses";
    };
    flaky = lib.mkOption {
      type = lib.types.listOf lib.types.attrs;
      readOnly = true;
      default = if cfg.report == null then [ ] else derived.flaky;
      description = "its adapters known to fail, and why (nix/lib/flaky-nics.json)";
    };
  };

  config = lib.mkIf (cfg.report != null) {
    boot.initrd.availableKernelModules = derived.initrdModules;
    warnings = derived.warnings;
    assertions = [
      {
        # the house's Wi-Fi profile is for the adapter the box has
        assertion = config.dd.home.wifi == null || config.dd.home.wifi == derived.wifi;
        message = "dd.home.wifi is ${toString config.dd.home.wifi}, but the box reported ${
          if derived.wifi == null then "no Wi-Fi adapter" else derived.wifi
        }";
      }
    ];
    # the same, where the owner looks when the box misbehaves
    systemd.services.dd-net-check = lib.mkIf (derived.warnings != [ ]) {
      description = "Say what is known to fail in this box's network hardware";
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = lib.concatMapStrings (w: ''
        echo ${lib.escapeShellArg "dd-net-check: ${w}"}
      '') derived.warnings;
    };
  };
}
