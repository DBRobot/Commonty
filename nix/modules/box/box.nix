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
  # a hardware fact, set in the box's hardware file with the driver
  options.dd.box.vaapi = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default = null;
    example = "/dev/dri/renderD128";
    description = "The render node of a GPU that encodes H.264 through VAAPI, if the box has one. The transcoder encodes there instead of on the cores.";
  };

  config.environment.etc."dd/plaintext".text =
    lib.concatStringsSep "\n" config.dd.box.plaintext + "\n";
}
