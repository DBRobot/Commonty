# The services the control plane switches (needs placed = true): their
# units on this box start only once /run/dd-placed/<service> exists, and
# dd-control puts it there and takes it away by the plan
# (servers/system/control/cmd/dd-control/placed.go). /run is empty at boot,
# so a placed service runs nowhere until the agreed state puts it somewhere.
# What this box carries - the placed services whose units it has - is
# /etc/dd-placed/carries.json, and its heartbeat, so the plan only puts a
# service where it can run.
{
  config,
  lib,
  ...
}:
let
  needs = import ../../lib/needs.nix { inherit lib; };
  dir = "/run/dd-placed";
  defined = kind: builtins.attrNames config.systemd.${kind};
  # a claim ending in * is every name that starts so
  match =
    names: c:
    if lib.hasSuffix "*" c then
      lib.filter (lib.hasPrefix (lib.removeSuffix "*" c)) names
    else
      lib.optional (builtins.elem c names) c;
  unitsOf =
    kind: svc: lib.unique (lib.concatMap (match (defined kind)) ([ svc ] ++ needs.${svc}.units));
  carries = lib.filterAttrs (_: u: u.services != [ ] || u.timers != [ ]) (
    lib.mapAttrs (s: _: {
      services = unitsOf "services" s;
      timers = unitsOf "timers" s;
    }) (lib.filterAttrs (_: n: n.placed) needs)
  );
  when = svc: { unitConfig.ConditionPathExists = "${dir}/${svc}"; };
in
{
  config = lib.mkIf config.dd.control.enable {
    environment.etc."dd-placed/carries.json".text = builtins.toJSON (
      lib.mapAttrs (_: u: map (x: "${x}.service") u.services ++ map (x: "${x}.timer") u.timers) carries
    );
    systemd.tmpfiles.rules = [ "d ${dir} 0755 root root -" ];
    systemd.services = lib.mkMerge (
      lib.mapAttrsToList (s: u: lib.genAttrs u.services (_: when s)) carries
    );
    systemd.timers = lib.mkMerge (lib.mapAttrsToList (s: u: lib.genAttrs u.timers (_: when s)) carries);
  };
}
