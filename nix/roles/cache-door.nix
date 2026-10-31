{
  # the home controllers' updates (modules/smarthome/cache-door.nix)
  imports = [ ../modules/smarthome/cache-door.nix ];
  dd.cacheDoor.enable = true;
}
