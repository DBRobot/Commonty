# Every systemd service and timer our modules put on a box belongs to a
# service in the needs (nix/modules/*/needs.nix: its name, or one of its
# `units`), so placement, the fit check and the privacy labels see all of
# it. And none of our timers counts from boot: a switch that adds or
# changes one starts it long after boot, and OnBootSec then runs it at once,
# mid-switch, where a failure rolls the release back (release 224).
# Fails while the flake is evaluated, so flake_check names the unit.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  needs = import ../lib/needs.nix { inherit lib; };
  root = toString self.outPath;
  # a definition's unit names, through mkIf, mkMerge and mkOverride
  names =
    v:
    if !(builtins.isAttrs v) then
      [ ]
    else if (v._type or null) == "if" then
      (if v.condition then names v.content else [ ])
    else if (v._type or null) == "merge" then
      lib.concatMap names v.contents
    else if
      builtins.elem (v._type or null) [
        "override"
        "order"
      ]
    then
      names v.content
    else
      builtins.attrNames v;
  # what this repository's files define, of the units the box has
  ours =
    box: kind:
    let
      c = self.nixosConfigurations.${box};
    in
    lib.filter (n: c.config.systemd.${kind} ? ${n}) (
      lib.unique (
        lib.concatMap (d: names d.value) (
          lib.filter (
            d: lib.hasPrefix root (toString d.file)
          ) c.options.systemd.${kind}.definitionsWithLocations
        )
      )
    );
  claims = lib.unique (
    builtins.attrNames needs ++ lib.concatMap (n: n.units) (builtins.attrValues needs)
  );
  claimed =
    u:
    builtins.any (
      c: if lib.hasSuffix "*" c then lib.hasPrefix (lib.removeSuffix "*" c) u else c == u
    ) claims;
  boxes = builtins.attrNames (lib.importJSON ../../fleet/boxes.json);
  problems = lib.concatMap (
    box:
    let
      c = self.nixosConfigurations.${box}.config;
    in
    map (u: "${box}: ${u} is in no service's needs (a `units` entry in nix/modules/*/needs.nix)") (
      lib.filter (u: !claimed u) (lib.unique (ours box "services" ++ ours box "timers"))
    )
    ++ map (
      t:
      "${box}: the timer ${t} counts from boot (OnBootSec); count from the timer starting (OnActiveSec)"
    ) (lib.filter (t: c.systemd.timers.${t}.timerConfig ? OnBootSec) (ours box "timers"))
  ) boxes;
in
if problems == [ ] then
  pkgs.runCommand "needs-units" { }
    "echo 'every unit has a service, no timer counts from boot' && touch $out"
else
  throw ("\n" + lib.concatStringsSep "\n" problems)
