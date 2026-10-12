# The home controller's two systems (docs/dev/home-controller.md), both
# aarch64-linux and the same in every house:
#
#   home-controller        what a controller runs, from its locked partition;
#                          what releases carry and dd-agent moves to
#   home-controller-seed   what the card image boots the first time, from its
#                          plain seed partition: it locks the card and installs
#                          home-controller into it (first-boot.nix)
{
  self,
  nixpkgs,
  tang,
}:
let
  common = [
    ./system.nix
    {
      _module.args = { inherit self tang; };
      nixpkgs.hostPlatform = "aarch64-linux";
    }
  ];
  home-controller = nixpkgs.lib.nixosSystem { modules = common ++ [ ./locked.nix ]; };
in
{
  inherit home-controller;
  home-controller-seed = nixpkgs.lib.nixosSystem {
    modules = common ++ [
      ./image.nix
      ./first-boot.nix
      (
        { lib, ... }:
        {
          dd.controller.installs = home-controller.config.system.build.toplevel;
          # the seed holds nothing of a home: no door, no Home Assistant, no
          # network, no releases; it locks the card and hands over
          dd.smarthome.enable = lib.mkForce false;
          dd.door.enable = lib.mkForce false;
          dd.net.enable = lib.mkForce false;
          dd.agent.enable = lib.mkForce false;
        }
      )
    ];
  };
}
