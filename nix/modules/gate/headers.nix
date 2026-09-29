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
    add_header Content-Security-Policy "frame-ancestors 'self' https://*.${base}" always;
  '';
  ours = lib.concatStringsSep "; " [
    "default-src 'self'"
    # dd_web is WebAssembly: compiling it is what 'wasm-unsafe-eval' allows
    "script-src 'self' 'wasm-unsafe-eval'"
    "style-src 'self' 'unsafe-inline'"
    "img-src 'self' data: blob: https:"
    "media-src 'self' blob: https://*.${base}"
    "connect-src 'self' https://*.${base} wss://*.${base}"
    "font-src 'self' data:"
    "frame-src 'self' blob: https://*.${base}"
    "worker-src 'self' blob:"
    "object-src 'none'"
    "base-uri 'self'"
    "form-action 'self' https://*.${base}"
    "frame-ancestors 'self' https://*.${base}"
    "report-uri /_dd/csp"
  ];
  # every location, not the server: nginx drops a server's add_header in
  # any location with one of its own, and upstream modules add their own
  location =
    { config, ... }:
    let
      to = if config.proxyPass == null then "" else config.proxyPass;
      # the gate's pages, and the games manager's, are ours
      gate = lib.hasInfix gatePort to || lib.hasInfix "dd-games.sock" to;
      vaultwarden = lib.hasInfix ":${toString vaultPort}" to;
    in
    {
      config.extraConfig = lib.mkAfter (
        if vaultwarden then
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
      lib.types.submodule {
        options.locations = lib.mkOption {
          type = lib.types.attrsOf (lib.types.submodule location);
        };
      }
    );
  };
}
