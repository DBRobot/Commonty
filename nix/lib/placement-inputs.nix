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
    can = lib.mapAttrs (_: c: c.score) capability.boxes.${n}.can;
  }) list;
  services = lib.mapAttrs (s: n: {
    enabled = n.everyBox || (builtins.elem s brought && on n);
    inherit (n) everyBox spread placed;
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
  roles = lib.mapAttrs (_: r: r.services or [ ]) roles;
  # a move must win by base, and by perGiB for each GiB it carries: a
  # stateless service moves for a clearly better box, 100G of photos for
  # one better by a whole point of suits more
  damping = {
    base = 0.3;
    perGiB = 0.01;
  };
}
