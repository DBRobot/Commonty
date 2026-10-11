# Where services may run, from their needs (nix/lib/needs.nix), each box's
# signed report (fleet/boxes/<box>/hardware.json), its owner's box.json, and
# the roles each box has (fleet/boxes.json, each role's services in
# fleet/roles.json). For every box: the services its roles bring, what each
# of them requires of the box, what must share the box with them, and
# whether their memory and storage fit. For every role: the boxes it could
# be added to as things stand. nix/tests/placement.nix fails on any problem;
# `.#placement` is the whole answer as JSON, what a placer chooses from.
{ lib }:
let
  needs = import ./needs.nix { inherit lib; };
  roles = lib.filterAttrs (_: v: builtins.isAttrs v) (lib.importJSON ../../fleet/roles.json);
  boxes = lib.importJSON ../../fleet/boxes.json;
  hwOf = n: lib.importJSON (../../fleet/boxes + "/${n}/hardware.json");
  ownOf =
    n:
    let
      p = ../../fleet/boxes + "/${n}/box.json";
    in
    if builtins.pathExists p then lib.importJSON p else { };

  unit = {
    "" = 1;
    K = 1024;
    M = 1024 * 1024;
    G = 1024 * 1024 * 1024;
    T = 1024 * 1024 * 1024 * 1024;
  };
  bytes =
    s:
    if s == null || s == "0" then
      0
    else
      let
        m = builtins.match "([0-9.]+)([KMGT]?)" s;
      in
      builtins.fromJSON (lib.elemAt m 0) * unit.${lib.elemAt m 1};
  gib = b: "${toString (b / (1024 * 1024 * 1024))}G";

  everyBox = lib.attrNames (lib.filterAttrs (_: n: n.everyBox) needs);
  servicesOf = rs: lib.unique (everyBox ++ lib.concatMap (r: roles.${r}.services or [ ]) rs);

  req = s: needs.${s}.base.requires;
  # what one service lacks on a box: none means it may run there
  lacks =
    hw: own: s:
    let
      r = req s;
      tpm = hw.firmware.tpm or null;
    in
    lib.optional (
      (r.arch or [ ]) != [ ] && !(builtins.elem hw.cpu.arch r.arch)
    ) "${s} runs on ${lib.concatStringsSep "/" r.arch}, the box is ${hw.cpu.arch}"
    ++ lib.optional ((r.kvm or false) && !hw.virt.kvm) "${s} needs KVM"
    ++ lib.optional ((r.nestedKvm or false) && !hw.virt.nested) "${s} needs nested KVM"
    ++ lib.optional ((r.tpm or false) && !(tpm != null && tpm.p256)) "${s} needs a TPM 2.0 with P-256"
    ++ lib.optional ((r.uefi or false) && !hw.firmware.uefi) "${s} needs UEFI"
    ++ lib.optional (
      (r.wiredInitrd or false) && !(builtins.any (n: !n.wireless && n.initrd) hw.nics)
    ) "${s} needs a wired NIC whose driver is in the initrd"
    ++ lib.optional (
      (r.threads or 0) > hw.cpu.threads
    ) "${s} needs ${toString r.threads} threads, the box has ${toString hw.cpu.threads}"
    ++ lib.optional ((r.gpu or null) != null && (hw.gpus or [ ]) == [ ]) "${s} needs a GPU"
    ++ lib.optional (
      (r.disk or null) != null && r.disk.kind == "ssd" && !(builtins.any (d: d.kind != "hdd") hw.disks)
    ) "${s} needs an SSD"
    ++ lib.optional (
      (r.place or "any") == "house" && !(own ? house)
    ) "${s} runs in a house (box.json house)";

  # memory: what it uses plus what it holds regardless; storage: what it
  # uses or the room it asks for, the larger
  memoryOf = s: bytes needs.${s}.base.uses.memory + bytes ((req s).memory.fixed or null);
  storageOf =
    s: lib.max (bytes needs.${s}.base.uses.storage) (bytes (((req s).disk or { }).size.min or null));
  sum = f: builtins.foldl' (a: s: a + f s) 0;

  # everything wrong with a box carrying these services
  problemsOn =
    name: services:
    let
      hw = hwOf name;
      own = ownOf name;
      memory = sum memoryOf services;
      storage = sum storageOf services;
      pools = builtins.foldl' (a: p: a + p.bytes) 0 (hw.pools or [ ]);
    in
    lib.concatMap (lacks hw own) services
    ++ lib.concatMap (
      s:
      map (d: "${s} needs ${d} on the same box") (
        lib.filter (d: needs ? ${d} && needs.${s}.base.depends.${d}.local && !(builtins.elem d services)) (
          builtins.attrNames needs.${s}.base.depends
        )
      )
    ) services
    ++ lib.optional (
      memory > hw.memoryBytes
    ) "its services use ${gib memory} of memory, it has ${gib hw.memoryBytes}"
    ++ lib.optional (
      pools > 0 && storage > pools
    ) "its services want ${gib storage} of storage, its pools hold ${gib pools}";

  names = builtins.attrNames boxes;
  on = n: servicesOf boxes.${n}.roles;

  # at most so many boxes, or so many in a house
  counted = lib.concatMap (
    s:
    let
      at = lib.filter (n: builtins.elem s (on n)) names;
      inst = needs.${s}.instances;
      house = needs.${s}.perHouse;
      sites = lib.unique (map (n: boxes.${n}.siteId or n) at);
      inSite = site: lib.filter (n: (boxes.${n}.siteId or n) == site) at;
    in
    lib.optional (inst != null && builtins.length at > inst)
      "${s} runs on ${toString (builtins.length at)} boxes (${lib.concatStringsSep ", " at}), at most ${toString inst}"
    ++ lib.concatMap (
      site:
      lib.optional (house != null && builtins.length (inSite site) > house)
        "${s} runs ${toString (builtins.length (inSite site))} times in one house, at most ${toString house}"
    ) sites
  ) (builtins.attrNames needs);
in
{
  boxes = lib.genAttrs names (
    n:
    let
      services = on n;
      hw = hwOf n;
    in
    {
      inherit (boxes.${n}) roles;
      inherit services;
      memory = {
        used = sum memoryOf services;
        total = hw.memoryBytes;
      };
      storage = {
        used = sum storageOf services;
        total = builtins.foldl' (a: p: a + p.bytes) 0 (hw.pools or [ ]);
      };
      problems = problemsOn n services;
    }
  );
  # each role, the boxes it could be added to (or already runs on) as
  # things stand: every service it brings fits there beside what is there
  roles = lib.mapAttrs (
    r: _: lib.filter (n: problemsOn n (lib.unique (on n ++ servicesOf [ r ])) == [ ]) names
  ) roles;
  problems = lib.concatMap (n: map (p: "${n}: ${p}") (problemsOn n (on n))) names ++ counted;
}
