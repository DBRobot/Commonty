# Our session cookie reaches the gate and no one else (verify.nix has the
# map that takes it out). Every nginx location that proxies anywhere but the
# gate forwards the request without it - Forgejo, Headscale, Thanos, Garage,
# Photos, whatever a module adds next - so a backend that is compromised
# never holds a member's live session.
{ config, lib, ... }:
let
  gatePort = ":${toString config.dd.verify.port}";
  location =
    { config, ... }:
    {
      config.extraConfig = lib.mkIf (
        config.proxyPass != null && !lib.hasInfix gatePort config.proxyPass
      ) (lib.mkAfter "proxy_set_header Cookie $dd_cookie_stripped;");
    };
in
{
  options.services.nginx.virtualHosts = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule {
        options.locations = lib.mkOption {
          type = lib.types.attrsOf (lib.types.submodule location);
        };
      }
    );
  };
}
