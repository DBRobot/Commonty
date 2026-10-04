# Does each box's role list fit what the box reported about itself, and
# what its owner chose? fleet/roles.json says what each role needs. Returns
# the problems, as sentences: none means it fits. Placement will choose
# roles with the same rules; today the check (tests/role-fit.nix) only
# keeps fleet/boxes.json honest.
{ lib }:
{
  roles,
  boxes,
  dir,
}:
let
  gib = 1024 * 1024 * 1024;
  # one requirement against one box's report: null when it holds
  need =
    hw: k: v:
    {
      arch = if hw.cpu.arch == v then null else "is ${hw.cpu.arch}, not ${v}";
      uefi = if hw.firmware.uefi == v then null else "does not boot UEFI";
      tpmP256 =
        if hw.firmware.tpm != null && hw.firmware.tpm.p256 then null else "has no TPM 2.0 with P-256";
      wiredInitrd =
        if lib.any (n: !n.wireless && n.initrd) hw.nics then
          null
        else
          "has no wired NIC whose driver is in the initrd (the disks cannot unlock)";
      memoryGiB =
        if hw.memoryBytes >= v * gib then
          null
        else
          "has ${toString (hw.memoryBytes / gib)}G of memory, not ${toString v}G";
      threads = if hw.cpu.threads >= v then null else "has ${toString hw.cpu.threads} threads, not ${toString v}";
      cpuFlags =
        let
          missing = lib.filter (f: !(builtins.elem f hw.cpu.flags)) v;
        in
        if missing == [ ] then null else "lacks ${lib.concatStringsSep ", " missing}";
      kvm = if hw.virt.kvm then null else "has no /dev/kvm";
      nestedKvm = if hw.virt.nested then null else "cannot nest KVM";
    }
    .${k} or "has an unknown requirement ${k}";
  problemsOf =
    name: b:
    let
      hw = lib.importJSON (dir + "/${name}/hardware.json");
      own = lib.importJSON (dir + "/${name}/box.json");
      ofRole =
        r:
        let
          n = roles.${r} or null;
        in
        if n == null then
          [ "${name}: ${r} is not a role in fleet/roles.json" ]
        else
          lib.filter (p: p != null) (
            lib.mapAttrsToList (
              k: v:
              let
                why = need hw k v;
              in
              if why == null then null else "${name}: ${r} needs it, but it ${why}"
            ) (n.requires or { })
            ++ map (w: if builtins.elem w b.roles then null else "${name}: ${r} needs ${w} on the same box") (
              n."with" or [ ]
            )
            ++ map (
              f: if (own.house or { }) ? ${f} then null else "${name}: ${r} needs ${f} in its box.json"
            ) (n.box or [ ])
          );
    in
    lib.optional (!(builtins.elem "core" b.roles)) "${name}: every box runs core"
    ++ lib.concatMap ofRole b.roles;
  fleet = lib.concatLists (
    lib.mapAttrsToList (
      r: n:
      let
        on = lib.attrNames (lib.filterAttrs (_: b: builtins.elem r b.roles) boxes);
      in
      lib.optional (n ? perFleet && lib.length on != n.perFleet)
        "${r} runs on ${toString (lib.length on)} boxes (${lib.concatStringsSep ", " on}), not ${toString n.perFleet}"
    ) (removeAttrs roles [ "_" ])
  );
in
lib.concatLists (lib.mapAttrsToList problemsOf boxes) ++ fleet
