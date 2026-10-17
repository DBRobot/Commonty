# A member's computer is an instance of a template, from a record on the box:
# it boots its own disk, the owner signs in with their key, the system it
# built from its Nix file is the one the host made, /home outlives a
# restart, and it can't reach the box's loopback.
{ pkgs, lib, self, ... }:
{
  name = "vms";
  node.specialArgs = { inherit self; };
  nodes.box = {
    imports = [
      ./box.nix
      ../modules/vms/vms.nix
    ];
    dd.vms.enable = true;
    # the guests' filter is the firewall's (nix/modules/guests/egress.nix)
    networking.firewall.enable = lib.mkForce true;
    environment.systemPackages = [
      pkgs.curl
      pkgs.jq
      pkgs.openssh
      pkgs.python3
    ];
    # a 1G guest inside this one: needs a builder with kvm, nesting on
    virtualisation.memorySize = 3072;
    virtualisation.cores = 2;
    virtualisation.diskSize = 16384;
  };
}
