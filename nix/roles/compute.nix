{ config, ... }:
{
  # Members' own computers (nix/modules/computers/computers.nix, docs/dev/remote-computers.md):
  # a VM each, with its own disks, made from a config they save. That a
  # given computer exists is the box's state, not the repo's.
  imports = [ ../modules/computers/computers.nix ];

  dd.computers.enable = true;

  dd.home.services = [
    {
      name = "Remote computers";
      blurb = "A remote computer of your own: SSH in, install what you need.";
      url = "https://compute.${config.dd.domain}/";
      icon = "computer";
      color = "#3b6f8f";
      rank = 36;
      demo = "read";
    }
  ];

  # the records and configs: without them a restored disk is no one's
  # computer. The disks themselves go to snapshots (docs/dev/remote-computers.md),
  # not the nightly backup
  dd.backup.paths = [
    "/var/lib/dd-computers/records"
    "/var/lib/dd-computers/configs"
  ];
}
