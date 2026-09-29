{ config, lib, ... }:
{
  # Ad blocking at home: Pi-hole for this box's house network. The house
  # (its address and who lives there) is the host's; see modules/adblock.
  imports = [ ../modules/adblock/pihole.nix ];
  dd.adblock = {
    enable = true;
    lan = lib.mkDefault (lib.head (lib.splitString "/" config.dd.home.address));
  };
}
