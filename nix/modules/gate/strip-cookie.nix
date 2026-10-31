# Our session cookie, under either name, reaches the gate and no one else
# (verify.nix has the maps that take it out). Every nginx location that proxies anywhere but the
# gate forwards the request without it - Forgejo, Headscale, Thanos, Garage,
# Photos, whatever a module adds next - so a backend that is compromised
# never holds a member's live session.
{ config, lib, ... }:
let
  gatePort = ":${toString config.dd.verify.port}";
  location =
    { config, ... }:
    {
      # A location that must send less sets this, never a Cookie line of its
      # own: nginx sends every proxy_set_header Cookie it is given, so the
      # backend would still get every cookie but ours.
      options.ddCookie = lib.mkOption {
        type = lib.types.str;
        default = "$dd_cookie_stripped";
        description = "The Cookie this location sends on: $dd_cookie_stripped or a variable made from it, never $http_cookie; empty sends none.";
      };
      config.extraConfig = lib.mkMerge [
        (lib.mkIf (config.proxyPass != null && !lib.hasInfix gatePort config.proxyPass) (
          lib.mkAfter ''proxy_set_header Cookie "${config.ddCookie}";''
        ))
        # and to the gate, the name this server block was asked under, never
        # one the client sent (the gate also checks it against its own names)
        (lib.mkIf (config.proxyPass != null && lib.hasInfix gatePort config.proxyPass) (
          lib.mkAfter "proxy_set_header X-Original-Host $host;"
        ))
        # and who is asking, as nginx saw the connection - on the public
        # door, the visitor's address the tunnel reports (real_ip, trusted
        # from the tunnel alone) - set here so whatever a client sent under
        # that name is overwritten. The gate's limits count by it
        (lib.mkIf (config.proxyPass != null && lib.hasInfix gatePort config.proxyPass) (
          lib.mkAfter "proxy_set_header X-Dd-Peer $remote_addr;"
        ))
      ];
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
