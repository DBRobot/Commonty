# A freshly imaged box runs what it carries: the real joined box
# (lib.joinedBoxModules) from the box.json the installer writes and a report
# with room for it, so by its capability it carries the games. Its flag up by
# hand, the plan not yet giving a joined box what is its owner's alone
# (placement-inputs joined.own), the manager runs on the dataset the installer
# named, and nginx serves its name. Its reach from a member's device, and its
# certificate, are not here.
{
  pkgs,
  lib,
  self,
  ...
}:
{
  name = "joined-games";
  node.specialArgs = { inherit self; };
  helpers = true;
  nodes.box =
    { lib, ... }:
    {
      imports = self.lib.joinedBoxModules {
        name = "joined-games";
        dir = ./joined-games-vm;
      };
      virtualisation.tpm.enable = true;
      virtualisation.memorySize = 3072;
      virtualisation.emptyDiskImages = [ 2048 ];
      networking.networkmanager.enable = lib.mkForce false;
      dd.unlock.waits.enrolment = 1;
      # no fleet here: its flags are the test's, and it stays on this system
      systemd.services.dd-control.enable = lib.mkForce false;
      systemd.services.dd-agent.enable = lib.mkForce false;
      systemd.timers.dd-agent.enable = lib.mkForce false;
      environment.systemPackages = [ pkgs.curl ];
    };
}
