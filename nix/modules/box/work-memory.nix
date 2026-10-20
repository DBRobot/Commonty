# Builds and CI share one budget. node2 (14G) ran out of memory four times
# on 2026-10-20: CI's VM tests beside a build order's (01:01-01:36), then a
# lint job's biome at 10G beside the app's LTO build (08:39). The nix
# daemon was held to its share, but a build order builds as root, outside
# the daemon, and a runner job's own processes were held to nothing, so
# together they took the whole box. In one slice they cannot: past its
# limit the kernel kills inside the slice, the biggest of them, and the
# box's own services never feel it. Each unit says what a kill does to it:
# a runner carries on (only the job whose process died fails), the rest
# stop as systemd's default has it.
{ config, lib, ... }:
let
  memoryBytes = config.dd.box.memoryBytes or 0;
  sizes = import ../../lib/work-memory.nix { inherit lib; } memoryBytes;
in
{
  options.dd.work.units = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule {
        options.oomPolicy = lib.mkOption {
          type = lib.types.enum [
            "continue"
            "stop"
            "kill"
          ];
          default = "stop";
          description = "what the unit does when the kernel kills one of its processes for memory";
        };
      }
    );
    default = { };
    description = "The services that build or run CI, by unit name: they run in work.slice, under the box's work budget (nix/lib/work-memory.nix).";
  };

  config = lib.mkIf (memoryBytes > 0) {
    systemd.slices.work = {
      description = "Builds and CI, held to what the box's own services leave";
      sliceConfig.MemoryMax = toString sizes.slice;
    };
    systemd.services = lib.mapAttrs (_: u: {
      serviceConfig = {
        Slice = "work.slice";
        OOMPolicy = u.oomPolicy;
      };
    }) config.dd.work.units;
  };
}
