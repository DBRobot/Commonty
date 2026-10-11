# What every box can run (nix/lib/capability.nix), from the needs, its
# signed report and its owner's box.json, against fleet/boxes.json: every
# box joins (it can run what every box runs), every service its roles bring
# is one it can run, and fleet/roles.json says truly what each role brings
# (a service with units is found on a box by them). Fails while the flake is
# evaluated, naming each problem; built, it is the whole table as JSON - what
# the control plane chooses from.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  needs = import ../lib/needs.nix { inherit lib; };
  list = lib.importJSON ../../fleet/boxes.json;
  roles = lib.filterAttrs (_: v: builtins.isAttrs v) (lib.importJSON ../../fleet/roles.json);
  capability = import ../lib/capability.nix {
    inherit lib needs;
    policy = lib.importJSON ../../fleet/policy.json;
    boxes = lib.mapAttrs (n: b: {
      hw = lib.importJSON ../../fleet/boxes/${n}/hardware.json;
      own = lib.importJSON ../../fleet/boxes/${n}/box.json;
      site = b.siteId or n;
      region = b.regionId or n;
    }) list;
  };
  every = lib.attrNames (lib.filterAttrs (_: n: n.everyBox) needs);
  brought = box: lib.unique (lib.concatMap (r: roles.${r}.services or [ ]) list.${box}.roles);
  claims = s: [ s ] ++ needs.${s}.units;
  matches = u: c: if lib.hasSuffix "*" c then lib.hasPrefix (lib.removeSuffix "*" c) u else c == u;
  running =
    box:
    let
      c = self.nixosConfigurations.${box}.config;
      units = builtins.attrNames c.systemd.services ++ builtins.attrNames c.systemd.timers;
    in
    lib.filter (s: builtins.any (u: builtins.any (matches u) (claims s)) units) (
      builtins.attrNames needs
    );
  problems = lib.concatMap (
    box:
    let
      t = capability.boxes.${box};
    in
    map (w: "${box}: cannot join the fleet: ${w}") t.why
    ++ lib.concatMap (
      s: map (w: "${box}: its roles bring ${s}, which it cannot run: ${w}") (t.cannot.${s} or [ ])
    ) (brought box)
    ++ map (s: "${box}: runs ${s}, which none of its roles brings (fleet/roles.json services)") (
      lib.filter (s: !(builtins.elem s (brought box ++ every))) (running box)
    )
  ) (builtins.attrNames list);
in
if problems == [ ] then
  pkgs.writeText "placement.json" (builtins.toJSON capability)
else
  throw ("\n" + lib.concatStringsSep "\n" problems)
