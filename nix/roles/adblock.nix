{ config, lib, ... }:
{
  # Ad blocking at home: Pi-hole for this box's house network. The house
  # (its address and who lives there) is the host's; see modules/adblock.
  imports = [
    ./_sops.nix
    ../modules/adblock/pihole.nix
    ../modules/smarthome/tang.nix
  ];
  # Pi-hole's api password is made on the box (modules/adblock/pihole.nix):
  # the gate holds it, nothing else does
  dd.adblock = {
    enable = true;
    lan = lib.mkDefault (lib.head (lib.splitString "/" config.dd.home.address));
  };
  # the house's home controllers open their cards through this box: the
  # house's main box, the one ad blocking already keeps on the house network
  dd.tang = {
    enable = true;
    house = config.dd.home.address;
  };
}
