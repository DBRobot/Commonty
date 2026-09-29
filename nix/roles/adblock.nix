{ config, lib, ... }:
{
  # Ad blocking at home: Pi-hole for this box's house network. The house
  # (its address and who lives there) is the host's; see modules/adblock.
  imports = [
    ./_sops.nix
    ../modules/adblock/pihole.nix
  ];
  # Pi-hole's api password, from sops: the gate holds it, nothing else does
  sops.secrets.pihole-api-password = { };
  sops.templates."pihole.env".content = ''
    FTLCONF_webserver_api_password=${config.sops.placeholder.pihole-api-password}
  '';
  dd.adblock = {
    enable = true;
    passwordEnv = config.sops.templates."pihole.env".path;
    passwordFile = config.sops.secrets.pihole-api-password.path;
    lan = lib.mkDefault (lib.head (lib.splitString "/" config.dd.home.address));
  };
}
