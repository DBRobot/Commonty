# What every box can run (nix/lib/capability.nix), from the needs, its
# signed report and its owner's box.json, against fleet/boxes.json: every
# box joins (it can run what every box runs), every service it carries is
# one it can run, and nix/roles/bundles.json says truly what each bundle
# brings (a service with units is found on a box by them), and each service's
# trust is one it chose and fits what it says it reads. Fails while the flake is
# evaluated, naming each problem; built, it is the whole table as JSON - what
# the control plane chooses from.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  inherit (import ../lib/fleet-placement.nix { inherit lib; })
    needs
    list
    capability
    built
    carries
    inputs
    ;
  every = lib.attrNames (lib.filterAttrs (_: n: n.everyBox) needs);
  # what a box is built with, from its capability (fleet-placement.nix built)
  brought = box: built.${box};
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
  # each box judges itself on the needs as JSON, the release's needs.json
  # (box/capability.nix): on the report the release has, it must come to
  # the release's own table
  judged = import ../lib/capability.nix {
    inherit lib;
    needs = builtins.fromJSON (builtins.toJSON needs);
    policy = lib.importJSON ../../fleet/policy.json;
    boxes = lib.mapAttrs (n: _: {
      hw = lib.importJSON ../../fleet/boxes/${n}/hardware.json;
      own = lib.importJSON ../../fleet/boxes/${n}/box.json;
    }) list;
  };
  scores = t: lib.mapAttrs (_: c: c.score) t.can;
  # a name follows one service (needs hosts), placed or under a lease: two
  # claiming one name, or one that nothing places, could not be followed
  named = lib.concatLists (lib.mapAttrsToList (s: n: map (h: { inherit s h; }) n.hosts) needs);
  hostProblems =
    lib.concatMap (
      h:
      let
        by = map (x: x.s) (lib.filter (x: x.h == h) named);
      in
      lib.optional (builtins.length by > 1)
        "${h}.<domain> is claimed by ${lib.concatStringsSep " and " by} (needs hosts): a name follows one service"
    ) (lib.unique (map (x: x.h) named))
    ++ map (x: "${x.s}: its hosts follow nothing, as it is neither placed nor under a lease") (
      lib.filter (x: (policyOn) && !(needs.${x.s}.placed || needs.${x.s}.lease)) named
    )
    # one box to point at, and what answers under it goes there too
    ++ map (s: "${s}: a name follows one box, so it runs once (instances = 1)") (
      lib.unique (map (x: x.s) (lib.filter (x: needs.${x.s}.instances != 1) named))
    );
  policyOn = (lib.importJSON ../../fleet/policy.json).failover or false;
  apart = lib.filterAttrs (_: n: !n.everyBox) needs;
  plaintext = n: lib.any (p: p.privacy == "plaintext") ([ n.base ] ++ builtins.attrValues n.variants);
  # who may run a service is said, and what it says of itself holds to it
  trustProblems =
    map (s: "${s}: no trust said (needs trust): whose boxes may run it is chosen, not a default") (
      builtins.attrNames (lib.filterAttrs (_: n: n.trust == null) apart)
    )
    ++ map (
      s: "${s}: trust any, but it declares plaintext: it would read someone's data on anyone's box"
    ) (builtins.attrNames (lib.filterAttrs (_: n: n.trust == "any" && plaintext n) apart))
    ++ map (s: "${s}: trust any, but the code holding people's secrets comes from it (servesCode)") (
      builtins.attrNames (lib.filterAttrs (_: n: n.trust == "any" && n.servesCode) apart)
    );
  # the console's password is a login's, no service's
  declaredSecrets = [
    "console-password-hash"
  ]
  ++ lib.concatMap (n: n.secrets) (builtins.attrValues needs);
  secretProblems = lib.concatMap (
    box:
    map (x: "${box}: reads ${x} from its own file, and no service says so (needs secrets)") (
      lib.filter (x: !(builtins.elem x declaredSecrets)) (
        builtins.attrNames self.nixosConfigurations.${box}.config.sops.secrets
      )
    )
  ) (builtins.attrNames list);
  # a box that joined on its own key: nothing it may be given reads another's
  # data or a secret from its file, or holds a name
  joinedProblems =
    map (s: "${s}: on a joined box, but it declares plaintext and is not its owner's") (
      lib.filter (s: plaintext needs.${s}) inputs.joined.services
    )
    ++ map (s: "${s}: joined.own, but trust ${toString needs.${s}.trust}") (
      lib.filter (s: needs.${s}.trust != "own" && needs.${s}.trust != "household") inputs.joined.own
    );
  # a box as the installer sets it up, on a real report, carries the games
  # (carries.json, what its heartbeat says and the plan gives from)
  fresh =
    (self.lib.joinedBox {
      name = "fresh";
      dir = ./placement-joined;
    }).config.environment.etc."dd-placed/carries.json".text;
  # the fixtures are the installer's box.json, with its member's word on the
  # line (LINE); whose it is comes in enlisted.json
  asInstalled =
    f: removeAttrs (lib.importJSON f) [ "internet" ] == lib.importJSON ../installer/box.json;
  freshProblems =
    map (f: "${f}/box.json is not what the installer writes (nix/installer/box.json)") (
      lib.filter (f: !asInstalled (./. + "/${f}/box.json")) [
        "placement-joined"
        "joined-games-vm"
      ]
    )
    ++ lib.optional (
      !(builtins.fromJSON fresh ? dd-games)
    ) "a freshly imaged box does not carry dd-games: ${fresh}";
  problems =
    hostProblems
    ++ trustProblems
    ++ freshProblems
    ++ secretProblems
    ++ joinedProblems
    ++ map (b: "${b}: judged on the box, its capability is not the release's") (
      lib.filter (b: scores judged.boxes.${b} != scores capability.boxes.${b}) (builtins.attrNames list)
    )
    ++ lib.concatMap (
      box:
      let
        t = capability.boxes.${box};
      in
      map (w: "${box}: cannot join the fleet: ${w}") t.why
      ++ lib.concatMap (
        s: map (w: "${box}: it carries ${s}, which it cannot run: ${w}") (t.cannot.${s} or [ ])
      ) carries.${box}
      ++ map (
        s: "${box}: runs ${s}, which none of its bundles brings (nix/roles/bundles.json services)"
      ) (lib.filter (s: !(builtins.elem s (brought box ++ every))) (running box))
    ) (builtins.attrNames list);
in
if problems == [ ] then
  pkgs.writeText "placement.json" (builtins.toJSON capability)
else
  throw ("\n" + lib.concatStringsSep "\n" problems)
