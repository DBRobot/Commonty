# What each box CAN run: for every service in the needs, whether this box
# meets what the service requires of it, and how well it suits it. Not what
# runs - the control plane decides that, from demand and what the boxes say
# live (docs/dev/control-plane.md) - and not load: a box is judged for each
# service alone. A pure function of signed inputs (the needs, each box's
# report and its owner's box.json, the fleet's policy), so every box computes
# the same table, and it ships in the release.
#
#   boxes.<name> = { hw = hardware.json; own = box.json; site = siteId; region = regionId; }
#
# Returns { boxes.<name> = { joins; why; can.<svc> = { score; for }; cannot.<svc> = [ why ];
#                           barred = [ svc ] (by its owner's choices or the policy, not hardware);
#                           owner (box.json's); memory; threads; pool (its report's) };
#           services.<svc> = [ { box; score; } ] best first;
#           sizes.<svc> = { memory; reserve; floor; } in GiB, as judged here }
{
  lib,
  needs,
  boxes,
  policy,
}:
let
  unit = {
    "" = 1.0;
    K = 1.0 / (1024 * 1024);
    M = 1.0 / 1024;
    G = 1.0;
    T = 1024.0;
  };
  # sizes in GiB
  gib =
    s:
    if s == null || s == "0" then
      0.0
    else
      let
        m = builtins.match "([0-9.]+)([KMGT]?)" s;
      in
      builtins.fromJSON (lib.elemAt m 0) * unit.${lib.elemAt m 1};
  bytesGiB = b: b / (1024.0 * 1024 * 1024);

  every = lib.attrNames (lib.filterAttrs (_: n: n.everyBox) needs);

  # what a service holds on its own: its use, what it keeps regardless, and
  # for one that loads a model, the largest it loads
  memoryOf = s: gib needs.${s}.base.uses.memory + reserveOf s;
  # what it keeps aside whatever it holds now: its fixed memory, and the
  # largest model it may load
  reserveOf =
    s:
    let
      n = needs.${s};
      fixed = v: gib (((v.requires or { }).memory or { }).fixed or null);
      models = map fixed (builtins.attrValues n.variants);
      biggest = builtins.foldl' lib.max 0.0 models;
    in
    fixed n.base + (if s == "llama-swap" then biggest else 0.0);
  floorOf =
    s:
    let
      b = needs.${s}.base;
    in
    lib.max (gib b.uses.storage) (gib ((b.requires.disk or { }).size.min or null));

  facts =
    name:
    let
      bx = boxes.${name};
      hw = bx.hw;
      own = bx.own;
      net = own.internet or { };
      pools = map (p: bytesGiB p.bytes) (hw.pools or [ ]);
    in
    {
      arch = hw.cpu.arch;
      threads = hw.cpu.threads;
      memory = bytesGiB hw.memoryBytes;
      # the most one dataset can hold: the largest pool
      pool = builtins.foldl' lib.max 0.0 pools;
      ssd = builtins.any (d: d.kind != "hdd") (hw.disks or [ ]);
      gpu = (hw.gpus or [ ]) != [ ];
      encode = lib.unique (lib.concatMap (g: g.encode or [ ]) (hw.gpus or [ ]));
      kvm = hw.virt.kvm or false;
      nested = hw.virt.nested or false;
      tpm = ((hw.firmware or { }).tpm or null) != null && hw.firmware.tpm.p256 or false;
      uefi = (hw.firmware or { }).uefi or false;
      wiredBoot = builtins.any (n: !n.wireless && n.initrd) (hw.nics or [ ]);
      mains = (hw.power or { }).mains or true;
      battery = (hw.power or { }).battery or false;
      # a line nobody has said is flat counts as metered
      metered = net.metered or true;
      owner = own.owner or null;
      household = (own.house or { }).household or [ ];
      inHouse = own ? house;
    };

  # what one requirement block asks that the box lacks
  lacksIn =
    f: r:
    lib.optional (
      (r.arch or [ ]) != [ ] && !(builtins.elem f.arch r.arch)
    ) "runs on ${lib.concatStringsSep "/" r.arch}, the box is ${f.arch}"
    ++ lib.optional ((r.kvm or false) && !f.kvm) "needs KVM"
    ++ lib.optional ((r.nestedKvm or false) && !f.nested) "needs nested KVM"
    ++ lib.optional ((r.tpm or false) && !f.tpm) "needs a TPM 2.0 with P-256"
    ++ lib.optional ((r.uefi or false) && !f.uefi) "needs UEFI"
    ++ lib.optional ((r.wiredInitrd or false) && !f.wiredBoot) "needs a wired NIC that works at boot"
    ++ lib.optional (
      (r.threads or 0) > f.threads
    ) "needs ${toString r.threads} threads, the box has ${toString f.threads}"
    ++ (
      let
        g = r.gpu or null;
      in
      lib.optional (g != null && !f.gpu) "needs a GPU"
      ++ lib.optional (
        g != null && f.gpu && builtins.any (e: !(builtins.elem e f.encode)) (g.encode or [ ])
      ) "needs a GPU that encodes ${lib.concatStringsSep ", " g.encode}"
    )
    ++ lib.optional (((r.disk or { }).kind or "any") == "ssd" && !f.ssd) "needs an SSD"
    ++ lib.optional ((r.place or "any") == "house" && !f.inHouse) "runs in a house";

  # everything that keeps service s off box `name`, before its dependencies
  ownLacks =
    name: s:
    let
      f = facts name;
      n = needs.${s};
      m = memoryOf s;
      d = floorOf s;
    in
    lacksIn f n.base.requires
    ++
      lib.optional (m > f.memory * 0.85)
        "needs ${toString (builtins.ceil m)}G of memory, the box has ${toString (builtins.floor f.memory)}G"
    ++
      lib.optional (d > f.pool * 0.85)
        "needs ${toString (builtins.ceil d)}G of storage, the box's largest pool holds ${toString (builtins.floor f.pool)}G"
    ++ ownerLacks name s;

  # what its owner chose (box.json) or the policy keeps off it, whatever its
  # hardware: the control plane never takes a box's own word over these
  ownerLacks =
    name: s:
    let
      f = facts name;
      n = needs.${s};
    in
    lib.optional (n.trust == "owner" && f.owner != policy.owner) "only on ${policy.owner}'s boxes"
    ++ lib.optional (
      n.trust == "household" && f.owner != policy.owner && f.household == [ ]
    ) "only on ${policy.owner}'s boxes or a household's"
    ++ lib.optional (
      n.traffic.class == "high" && f.metered
    ) "high traffic, and the line is metered (or not said to be flat)";

  # and through its dependencies: what it needs on the same box must be
  # able to run there too
  lacksBy =
    own: name:
    let
      go =
        seen: s:
        if builtins.elem s seen then
          [ ]
        else
          own name s
          ++ lib.concatMap (d: map (w: "${d} cannot run here: ${w}") (go (seen ++ [ s ]) d)) (
            lib.filter (d: needs ? ${d} && needs.${s}.base.depends.${d}.local) (
              builtins.attrNames needs.${s}.base.depends
            )
          );
    in
    go [ ];
  lacks = lacksBy ownLacks;

  # how well a box suits a service it can run, from what is known before
  # it runs (the live line, uptime and room are the control plane's)
  suits =
    name: s:
    let
      f = facts name;
      n = needs.${s};
      fast = n.base.faster;
      met = lacksIn f fast == [ ];
      wantsFast =
        lacksIn (
          facts name
          // {
            gpu = false;
            ssd = false;
            threads = 0;
            encode = [ ];
          }
        ) fast != [ ];
      m = memoryOf s;
      room = if f.memory > 0 then 1 - m / f.memory else 0;
      d = floorOf s;
    in
    lib.optional (wantsFast && met) {
      w = 1.0;
      why = "has what makes it faster";
    }
    ++ [
      {
        w = 0.5 * room;
        why = "memory to spare";
      }
    ]
    ++ lib.optional (n.alwaysOn && f.mains && !f.battery) {
      w = 0.5;
      why = "always on mains power";
    }
    ++ lib.optional (n.alwaysOn && !f.mains) {
      w = -1.0;
      why = "battery only";
    }
    ++ lib.optional (n.traffic.class == "medium" && f.metered) {
      w = -1.0;
      why = "metered line";
    }
    # threads past what it needs, for what is built to use them
    ++ lib.optional ((n.base.requires.threads or 0) > 0) {
      w = lib.min 1.0 (f.threads / (4.0 * n.base.requires.threads));
      why = "threads to spare";
    }
    ++ lib.optional (n.spread && d > 0) {
      w = lib.min 1.0 (f.pool / (d * 8));
      why = "room for the pool";
    };

  table = lib.genAttrs (builtins.attrNames boxes) (
    name:
    let
      blocked = lib.concatMap (s: map (w: "${s}: ${w}") (lacks name s)) every;
      others = lib.filter (s: !(builtins.elem s every)) (builtins.attrNames needs);
      why = lib.genAttrs others (lacks name);
      ok = lib.filterAttrs (_: w: w == [ ]) why;
    in
    {
      joins = blocked == [ ];
      why = blocked;
      can = lib.optionalAttrs (blocked == [ ]) (
        lib.mapAttrs (
          s: _:
          let
            parts = suits name s;
          in
          {
            score = builtins.foldl' (a: p: a + p.w) 0.0 parts;
            for = map (p: p.why) (lib.filter (p: p.w > 0) parts);
          }
        ) ok
      );
      cannot = lib.filterAttrs (_: w: w != [ ]) why;
      barred = lib.filter (s: lacksBy ownerLacks name s != [ ]) others;
      # its box.json owner: whose box a member's "own" computer may use;
      # and its hardware by its signed report, past which no word of its own
      # about its room is taken (GiB, a count, its largest pool in GiB)
      inherit (facts name)
        owner
        memory
        threads
        pool
        ;
    }
  );
in
{
  boxes = table;
  sizes = lib.mapAttrs (s: _: {
    memory = memoryOf s;
    reserve = reserveOf s;
    floor = floorOf s;
  }) needs;
  services = lib.genAttrs (lib.filter (s: !(builtins.elem s every)) (builtins.attrNames needs)) (
    s:
    lib.sort (a: b: a.score > b.score || (a.score == b.score && a.box < b.box)) (
      lib.concatMap (
        name:
        lib.optional (table.${name}.can ? ${s}) {
          box = name;
          inherit (table.${name}.can.${s}) score;
        }
      ) (builtins.attrNames table)
    )
  );
}
