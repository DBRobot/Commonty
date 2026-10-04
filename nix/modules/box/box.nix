{ config, lib, ... }:
{
  # No box is trusted. A service that reads people's plaintext exposes it
  # to whichever box runs it, and until that service is fixed that is a
  # fact to see, not a rule to enforce. Each such module registers itself
  # here; the box publishes the list as a metric and `dd box list` shows it,
  # so the work left is always in view and nothing is hidden behind a flag.
  options.dd.box.plaintext = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [ ];
    description = "Services on this box that read people's plaintext.";
  };
  # where the box is, at the two grains placement cares about; from
  # fleet/boxes.json, published as labels so a dashboard can group by them
  options.dd.box.tailnet = lib.mkOption {
    type = lib.types.str;
    default = "";
    description = "This box's tailnet address, from fleet/boxes.json: what a person types to reach something on it.";
  };
  options.dd.box.site = lib.mkOption {
    type = lib.types.str;
    default = "";
  };
  options.dd.box.region = lib.mkOption {
    type = lib.types.str;
    default = "";
  };
  # hardware facts, from the box's own report (modules/box/host.nix)
  options.dd.box.gpus = lib.mkOption {
    type = lib.types.listOf (
      lib.types.submodule {
        options = {
          render = lib.mkOption { type = lib.types.str; };
          driver = lib.mkOption { type = lib.types.str; };
          encode = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ ];
          };
        };
      }
    );
    default = [ ];
    description = "Every GPU the box has, whatever its maker: Vulkan is on for all of them.";
  };
  options.dd.box.s3 = lib.mkOption {
    type = lib.types.str;
    default = "http://127.0.0.1:3900";
    description = "the storage pool's S3 address for this box: its own garage, or a storage box's over the tailnet (flake.nix)";
  };
  options.dd.box.bulkPool = lib.mkOption {
    type = lib.types.str;
    default = "tank";
    description = "the pool for big, slow-to-matter data (box.json pools.bulk): a hard disk if the box has one";
  };
  options.dd.box.owner = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default = null;
    description = "whose box it is (box.json owner): services the policy keeps on the fleet owner's own hardware go only here";
  };
  options.dd.box.memoryBytes = lib.mkOption {
    type = lib.types.ints.unsigned;
    default = 0;
    description = "RAM, from the report: what sizes itself by memory starts here";
  };
  options.dd.box.threads = lib.mkOption {
    type = lib.types.ints.positive;
    default = 1;
    description = "CPU threads, from the report: the default for whatever sizes itself by them";
  };
  options.dd.box.vaapi = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default = null;
    example = "/dev/dri/renderD128";
    description = "The render node of a GPU that encodes H.264 through VAAPI, if the box has one. The transcoder encodes there instead of on the cores.";
  };

  config.environment.etc."dd/plaintext".text =
    lib.concatStringsSep "\n" config.dd.box.plaintext + "\n";
}
