{
  config,
  lib,
  pkgs,
  ...
}:
{
  # No box is trusted. A service that reads people's plaintext exposes it
  # to whichever box runs it, and until that service is fixed that is a
  # fact to see, not a rule to enforce. Each such module registers itself
  # here; the box publishes the list as a metric and `dd box list` shows it,
  # so the work left is always in view and nothing is hidden behind a flag.
  # Failover (fleet/policy.json failover): the services with one writer
  # follow their leases between the boxes able to run them, each box with
  # its own store and its own keys (docs/dev/control-plane.md). One switch
  # for the whole fleet; off, they run where their roles put them, as
  # before
  options.dd.failover = lib.mkOption {
    type = lib.types.bool;
    default = (lib.importJSON ../../../fleet/policy.json).failover or false;
    description = "the fleet's switch; a test sets it";
  };
  options.dd.box.plaintext = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [ ];
    description = "Services on this box that read people's plaintext.";
  };
  # where the box is, at the two grains placement cares about; from
  # fleet/boxes.json, published as labels so a dashboard can group by them
  options.dd.box.net = lib.mkOption {
    type = lib.types.str;
    default = "";
    description = "This box's address on the fleet's own network, from fleet/boxes.json (headscale keeps a node's address for good; a box that joins again as a new node gets a new one, and this changes with it).";
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
    description = "the storage pool's S3 address for this box: its own garage, or a storage box's over the fleet's network (flake.nix)";
  };
  options.dd.box.bulkPool = lib.mkOption {
    type = lib.types.str;
    default = "tank";
    description = "the pool for big, slow-to-matter data (box.json pools.bulk): a hard disk if the box has one";
  };
  # a box the release does not list: it joined the fleet on its own key,
  # and builds its own system (docs/dev/joining.md). Set by lib.joinedBox
  # alone, the only way such a box is built; any other system - a box of
  # fleet/boxes.json, a test's machine, the home controller - pulls the
  # release as it always has
  options.dd.box.joined = lib.mkOption {
    type = lib.types.bool;
    default = false;
  };
  options.dd.box.owner = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default = null;
    description = "whose box it is (box.json owner)";
  };
  options.dd.box.memoryBytes = lib.mkOption {
    type = lib.types.ints.unsigned;
    default = 0;
    description = "RAM, from the report: what sizes itself by memory starts here";
  };
  options.dd.box.disks = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [ ];
    description = "every disk the box has, by stable id, from its report: what smartd watches";
  };
  options.dd.box.metered = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = "the house line is metered or capped (box.json internet.metered, the owner's word): nothing measures it, and placement keeps heavy traffic off it";
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
  # the metric `dd box list` reads (L+: the directory is metrics.nix's)
  config.systemd.tmpfiles.rules = [
    "L+ /var/lib/dd-facts/plaintext.prom - - - - ${
      pkgs.writeText "plaintext.prom" (
        "# HELP dd_box_plaintext A service on this box that reads people's plaintext.\n# TYPE dd_box_plaintext gauge\n"
        + lib.concatMapStrings (
          s: "dd_box_plaintext{box=\"${config.networking.hostName}\",service=\"${s}\"} 1\n"
        ) config.dd.box.plaintext
      )
    }"
  ];
}
