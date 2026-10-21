# Every systemd service and timer our modules put on a box belongs to a
# service in the needs (nix/modules/*/needs.nix: its name, or one of its
# `units`), so placement, the fit check and the privacy labels see all of
# it. And none of our timers counts from boot: a switch that adds or
# changes one starts it long after boot, and OnBootSec then runs it at once,
# mid-switch, where a failure rolls the release back (release 224). And
# with failover on, a service that does not run on every box is switched by
# the control plane (needs placed, or a writer's lease), unless it has no
# unit of ours on any box, or is one of the few below, each for its reason.
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
      defs = c.options.systemd.${kind}.definitionsWithLocations;
      from =
        keep:
        lib.unique (
          lib.concatMap (d: names d.value) (lib.filter (d: keep (lib.hasPrefix root (toString d.file))) defs)
        );
      # an upstream unit we only add settings to (the firewall, nix-daemon)
      # is the module's that makes it, not a service of ours
      upstream = from (x: !x);
    in
    lib.filter (n: c.config.systemd.${kind} ? ${n} && !(builtins.elem n upstream)) (from (x: x));
  claims = lib.unique (
    builtins.attrNames needs ++ lib.concatMap (n: n.units) (builtins.attrValues needs)
  );
  claimed =
    u:
    builtins.any (
      c: if lib.hasSuffix "*" c then lib.hasPrefix (lib.removeSuffix "*" c) u else c == u
    ) claims;
  boxes = builtins.attrNames (lib.importJSON ../../fleet/boxes.json);
  # built in where their roles bring them and run there, not switched
  unswitched = {
    # the shared pool: every box's agent fetches its release's closures
    # through it, so it must not wait on the control plane agreeing
    garage = true;
    # the host's own units; each computer runs under its own lease
    # (computer-<id>, box/placed.nix leaves it to dd-control)
    computers = true;
    # the keys museum's and the vault's sign-ins are checked with, made on
    # every box that is built with them: the gate there reads them
    ente-web = true;
    vaultwarden-send = true;
  };
  switched = s: needs.${s}.everyBox || needs.${s}.placed || needs.${s}.lease || unswitched ? ${s};
  ownedBy =
    s: u:
    builtins.any (
      c:
      if lib.hasSuffix "*" c then
        lib.hasPrefix (lib.removeSuffix "*" c) u
      else if lib.hasSuffix "@" c then
        lib.hasPrefix c u
      else
        c == u
    ) ([ s ] ++ needs.${s}.units);
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
    ++ lib.optionals ((lib.importJSON ../../fleet/policy.json).failover or false) (
      map
        (
          s:
          "${box}: ${s} runs units here and does not run on every box: switch it (needs placed = true), or say in nix/tests/needs-units.nix why not"
        )
        (
          lib.filter (
            s:
            !(switched s)
            && builtins.any (ownedBy s) (
              builtins.attrNames c.systemd.services ++ builtins.attrNames c.systemd.timers
            )
          ) (builtins.attrNames needs)
        )
    )
  ) boxes;
in
if problems == [ ] then
  pkgs.runCommand "needs-units" { }
    "echo 'every unit has a service, no timer counts from boot' && touch $out"
else
  throw ("\n" + lib.concatStringsSep "\n" problems)
