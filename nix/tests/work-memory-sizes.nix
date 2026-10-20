# The work slice's limit comes out of a box's memory as it should
# (nix/lib/work-memory.nix), and every box puts its builds and CI under it,
# with a runner that outlives a killed job (modules/box/work-memory.nix).
{
  pkgs,
  lib,
  self,
  ...
}:
let
  gib = 1024 * 1024 * 1024;
  size = import ../lib/work-memory.nix { inherit lib; };
  sizes = [
    # node2's kind: 3G kept back
    {
      ram = 14 * gib;
      slice = 11 * gib;
    }
    # node1's kind: a fifth kept back
    {
      ram = 62 * gib;
      slice = 62 * gib - 62 * gib / 5;
    }
    # a small box keeps half, not all of it
    {
      ram = 4 * gib;
      slice = 2 * gib;
    }
  ];
  sizeProblems = lib.concatMap (
    t:
    let
      got = (size t.ram).slice;
    in
    lib.optional (
      got != t.slice
    ) "${toString t.ram} bytes of RAM: a work slice of ${toString got}, not ${toString t.slice}"
  ) sizes;
  boxProblems = lib.concatMap (
    box:
    let
      c = self.nixosConfigurations.${box}.config;
      s = c.systemd;
      ram = (lib.importJSON ../../fleet/boxes/${box}/hardware.json).memoryBytes;
      want = toString (size ram).slice;
      runner = "gitea-runner-${box}";
      inSlice =
        u:
        lib.optional (
          (s.services.${u}.serviceConfig.Slice or null) != "work.slice"
        ) "${box}: ${u} is not in work.slice";
    in
    lib.optional ((s.slices.work.sliceConfig.MemoryMax or null) != want)
      "${box}: work.slice MemoryMax is ${
        toString (s.slices.work.sliceConfig.MemoryMax or "unset")
      }, not ${want}"
    ++ inSlice "nix-daemon"
    ++ inSlice "dd-build"
    ++ lib.optionals (s.services ? ${runner}) (
      inSlice runner
      ++ lib.optional (
        s.services.${runner}.serviceConfig.OOMPolicy or null != "continue"
      ) "${box}: ${runner} does not carry on past a killed job (OOMPolicy)"
    )
  ) (builtins.attrNames (lib.importJSON ../../fleet/boxes.json));
  problems = sizeProblems ++ boxProblems;
in
if problems == [ ] then
  pkgs.runCommand "work-memory-sizes" { }
    "echo 'builds and CI are held to the work slice' && touch $out"
else
  throw ("\n" + lib.concatStringsSep "\n" problems)
