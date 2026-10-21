# What every page here tells the browser about itself.
#
# Every site: https only from now on (HSTS), no guessing a file's type
# (nosniff), no full address in the Referer, and no other site may put our
# pages in a frame. Passwords is left to Vaultwarden, which sends its own
# full set, but for HSTS.
#
# Our own pages - everything the gate serves, and the games pages - also
# get a content policy: scripts from this site alone, no inline script, data
# from our own names. Every page ran clean under it report-only (release
# 132) before it was enforced. Anything it blocks is reported to /_dd/csp,
# which the gate logs.
{ config, lib, ... }:
let
  base = config.dd.domain;
  gatePort = ":${toString config.dd.verify.port}";
  # Vaultwarden sends its own full set; only this is added to its answers
  vaultPort = 8222;
  hsts = ''
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
  '';
  plain = hsts + ''
    add_header X-Content-Type-Options nosniff always;
    add_header Referrer-Policy strict-origin-when-cross-origin always;
  '';
  common = plain + ''
    add_header Content-Security-Policy "frame-ancestors 'self'" always;
  '';
  ours = lib.concatStringsSep "; " [
    "default-src 'self'"
    # dd_web is WebAssembly: compiling it is what 'wasm-unsafe-eval' allows
    "script-src 'self' 'wasm-unsafe-eval'"
    "style-src 'self' 'unsafe-inline'"
    "img-src 'self' data: blob: https:"
    "media-src 'self' blob: https://*.${base}"
    # Movies & TV looks titles up at TMDB from the page, and fetches the
    # posters it finds there to keep them. The bare name too: home asks it
    # once for its front page (footer.js)
    "connect-src 'self' https://${base} https://*.${base} wss://*.${base} https://api.themoviedb.org https://image.tmdb.org"
    "font-src 'self' data:"
    "frame-src 'self' blob: https://*.${base}"
    "worker-src 'self' blob:"
    "object-src 'none'"
    "base-uri 'self'"
    "form-action 'self' https://*.${base}"
    "frame-ancestors 'self'"
    "report-uri /_dd/csp"
  ];
  # every location, not only the server: nginx drops a server's add_header
  # in any location with one of its own, and upstream modules add their own
  location =
    { config, ... }:
    let
      to = if config.proxyPass == null then "" else config.proxyPass;
      # the gate's pages, and the games manager's, are ours
      gate = lib.hasInfix gatePort to || lib.hasInfix "dd-games.sock" to;
      vaultwarden = lib.hasInfix ":${toString vaultPort}" to;
    in
    {
      # a location that must tell the browser something stricter than ours
      # (the forge's public door: modules/forge/door.nix)
      options.ddCsp = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "This location's whole Content-Security-Policy, in place of the one chosen by where it proxies.";
      };
      config.extraConfig = lib.mkAfter (
        if config.ddCsp != null then
          plain
          + ''
            add_header Content-Security-Policy "${config.ddCsp}" always;
          ''
        else if vaultwarden then
          hsts
        else if gate then
          plain
          + ''
            add_header Content-Security-Policy "${ours}" always;
          ''
        else
          common
      );
    };
in
{
  options.services.nginx.virtualHosts = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule (
        { config, ... }:
        {
          options.locations = lib.mkOption {
            type = lib.types.attrsOf (lib.types.submodule location);
          };
          # and the server: what no location takes (nginx's own 404, a
          # name with exact locations alone) is answered at this level, and
          # every location above sets its own, so this one reaches nothing
          # else. Vaultwarden's name keeps to its locations' HSTS alone.
          config.extraConfig = lib.mkAfter (
            if
              lib.any (
                l: lib.hasInfix ":${toString vaultPort}" (if l.proxyPass == null then "" else l.proxyPass)
              ) (lib.attrValues config.locations)
            then
              hsts
            else
              common
          );
        }
      )
    );
  };
}
