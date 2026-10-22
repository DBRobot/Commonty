# What the control plane places from (servers/system/control/place): each
# box's capability, scored (capability.nix), and what the fleet wants of
# each service - wanted when some box's roles in fleet/boxes.json bring it
# and the policy setting it waits on (needs when) is on,
# how many, spread or by house, the line it wants, its memory, what moving
# it carries, what must share its box - with fleet/roles.json to read what a
# box runs from the roles its heartbeat names. Ships as the release's
# placement-inputs.json; dd-control -placement reads it.
{
  lib,
  needs,
  capability,
  list,
  roles,
  policy,
}:
let
  on =
    n:
    n.when == null
    || (
      let
        v = policy.${n.when} or false;
      in
      v == true || (builtins.isList v && v != [ ])
    );
  brought = lib.unique (
    lib.concatMap (b: lib.concatMap (r: roles.${r}.services or [ ]) b.roles) (builtins.attrValues list)
  );
in
{
  boxes = lib.mapAttrs (n: b: {
    site = b.siteId or n;
    region = b.regionId or n;
    # members' computers only where a role of the box brings their host
    # (fleet/roles.json compute): a box that could run them but does not
    # carry the units has none placed on it nor its lease
    can = lib.mapAttrs (_: c: c.score) (
      lib.filterAttrs (
        s: _: s != "computers" || builtins.elem s (lib.concatMap (r: roles.${r}.services or [ ]) b.roles)
      ) capability.boxes.${n}.can
    );
    # what its owner's box.json or the policy keep off it: a box's own
    # judgment in its heartbeat (box/capability.nix) never adds these
    # and whose box it is, for a member's computer kept to their own boxes
    inherit (capability.boxes.${n}) barred owner threads;
    memoryGiB = capability.boxes.${n}.memory;
    poolGiB = capability.boxes.${n}.pool;
  }) list;
  services = lib.mapAttrs (s: n: {
    enabled = n.everyBox || (builtins.elem s brought && on n);
    inherit (n)
      everyBox
      spread
      lease
      moves
      ;
    placed = n.placed || n.lease;
    perHouse = if n.perHouse == null then 0 else n.perHouse;
    instances = if n.instances == null then 0 else n.instances;
    inherit (n.traffic) class upMbps downMbps;
    memoryGiB = capability.sizes.${s}.memory;
    reserveGiB = capability.sizes.${s}.reserve;
    # until the boxes report what each holds, its floor
    storageGiB = capability.sizes.${s}.floor;
    local = lib.filter (d: needs ? ${d} && n.base.depends.${d}.local) (
      builtins.attrNames n.base.depends
    );
  }) needs;
  # each bucket a service writes: how many stores keep it, and whose it is
  buckets = lib.foldl' (a: b: a // b) { } (
    lib.mapAttrsToList (
      s: n:
      lib.mapAttrs (_: b: {
        inherit (b) copies;
        service = s;
      }) n.buckets
    ) needs
  );
  roles = lib.mapAttrs (_: r: r.services or [ ]) roles;
  # what a box that joined on its own key may be given (place.JoinedRules):
  # by the policy's trust tiers, a placed service any box may run, that
  # hands nobody the code holding their secrets, that the owner keeps to
  # none of their own hardware, with no lease. Off (attested false) until
  # a joined box's hardware and boot are attested in the agreed state
  # (docs/dev/attestation.md): till then it holds copies only
  # (and what it needs beside it is every box's, or allowed too)
  joined =
    let
      tier = builtins.attrNames (
        lib.filterAttrs (
          s: n:
          n.placed
          && !n.lease
          && !n.everyBox
          && n.trust == "any"
          && !n.servesCode
          && !builtins.elem s (policy.ownHardware or [ ])
        ) needs
      );
      localOf =
        s:
        lib.filter (d: needs ? ${d} && needs.${s}.base.depends.${d}.local) (
          builtins.attrNames needs.${s}.base.depends
        );
      keep =
        ss:
        let
          kept = lib.filter (s: lib.all (d: needs.${d}.everyBox || builtins.elem d ss) (localOf s)) ss;
        in
        if kept == ss then ss else keep kept;
    in
    {
      attested = false;
      services = keep tier;
    };
  # a move must win by base, and by perGiB for each GiB it carries: a
  # stateless service moves for a clearly better box, 100G of photos for
  # one better by a whole point of suits more
  damping = {
    base = 0.3;
    perGiB = 0.01;
  };
}
