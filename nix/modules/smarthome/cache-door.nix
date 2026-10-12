# The home controllers' updates (docs/dev/home-controller.md): this box's
# own nix store, read-only, at cache.<domain> on the Commonty network. A
# controller holds no fleet secret, so it cannot read the cache bucket; this
# store already holds every path of the controller's system, because every
# box realises it to vouch for a release (dd-attest). What it serves is
# built from the public repository and nothing else: the store holds no
# secrets. The controller's dd-agent checks each closure against the signed
# release, so this door is trusted for nothing.
{ config, lib, ... }:
let
  port = 5005;
in
{
  options.dd.cacheDoor.enable = lib.mkEnableOption "the controllers' read-only cache door";

  config = lib.mkIf config.dd.cacheDoor.enable {
    services.harmonia = {
      enable = true;
      settings.bind = "127.0.0.1:${toString port}";
    };
    # on the network alone: not in dd.public.hosts, and nothing but reading
    services.nginx.virtualHosts."cache.${config.dd.domain}" = {
      useACMEHost = config.dd.domain;
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://127.0.0.1:${toString port}";
        extraConfig = ''
          limit_except GET HEAD { deny all; }
          proxy_buffering off;
        '';
      };
    };
  };
}
