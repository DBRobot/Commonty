# What the control plane places from (servers/system/control/place): each
# box's capability, scored (capability.nix), and what the fleet wants of
# each service - wanted when some box is built with it (fleet-placement.nix
# built) and the policy setting it waits on (needs when) is on,
# how many, spread or by house, the line it wants, its memory, what moving
# it carries, what must share its box - and what each box is built with, so
# what runs there unswitched is known by the release, not by its word.
# Ships as the release's placement-inputs.json; dd-control -placement reads it.
{
  lib,
  needs,
  capability,
  list,
  built,
  policy,
  # a shared bucket's home store, by bucket (fleet-placement.nix homes)
  homes ? { },
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
  brought = lib.unique (lib.concatLists (builtins.attrValues built));
in
{
  boxes = lib.mapAttrs (n: b: {
    site = b.siteId or n;
    region = b.regionId or n;
    # what it can run and is built with: a box that could run a service
    # but has not its units (members' computers' host among them) has none
    # placed on it, nor its lease
    can = lib.mapAttrs (_: c: c.score) (
      lib.filterAttrs (s: _: builtins.elem s built.${n}) capability.boxes.${n}.can
    );
    # what it is built with, by the release: what runs there unswitched
    built = built.${n};
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
  # each bucket a service writes: how many stores keep it, and whose it is;
  # one a box writes alone (perBox) is that box's, wherever its service runs,
  # and a shared one is its home's
  buckets = lib.foldl' (a: b: a // b) { } (
    lib.mapAttrsToList (
      s: n:
      lib.foldl' (a: b: a // b) { } (
        lib.mapAttrsToList (
          name: b:
          if b.perBox then
            lib.mapAttrs' (
              box: _:
              lib.nameValuePair "${name}-${box}" {
                inherit (b) copies;
                service = s;
                inherit box;
              }
            ) list
          else
            {
              ${name} = {
                inherit (b) copies;
                service = s;
              }
              // lib.optionalAttrs (homes ? ${name}) { box = homes.${name}; };
            }
        ) n.buckets
      )
    ) needs
  );
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
          s: n: n.placed && !n.lease && !n.everyBox && n.trust == "any" && !n.servesCode
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
