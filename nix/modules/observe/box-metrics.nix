# A box's own metrics, from the box itself: metrics.<box>.<domain>, read-only
# queries of its own live data for its owner and the admin alone, asked by
# the Metrics page (servers/observe/web) from wherever that page is. Live,
# where the bucket's copy is hours behind; and no box answers for another.
#
# Only once sign-in is the id Worker's and the zone the DNS Worker's: the
# box's gate takes a browser's sign-in by the hand-off, its certificate is
# asked of the DNS Worker, and the DNS Worker points *.<box>.<domain> here.
# The box that gathers every box's metrics (observe, Thanos) answers at
# metrics.<domain> as before.
{
  config,
  lib,
  options,
  ...
}:
let
  base = config.dd.domain;
  box = config.networking.hostName;
  host = "metrics.${box}.${base}";
  port = config.dd.verify.port;
  # the gathering box keeps metrics.<domain>; the box with the front door
  # (gate/public.nix) is the full gate already
  on =
    config.dd.idWorker != ""
    && config.dd.idWorkerStage == "on"
    && config.dd.dnsWorker != ""
    && !(options.dd ? public)
    && config.dd.thanos.sidecars == [ ];
in
{
  options.dd.boxMetrics.host = lib.mkOption {
    type = lib.types.str;
    default = if on then host else "";
    readOnly = true;
    description = "this box's own metrics name, or empty";
  };

  config = lib.mkIf on {
    # this box's pages alone: its owner and the admin, and the hand-off
    dd.verify.role = lib.mkForce "box";
    dd.verify.hosts = lib.mkForce [ "metrics.${box}" ];
    dd.acme.enable = true;

    # a query service over this box's own sidecar alone: its live data, with
    # the box label every box's carries (its Prometheus's own answers do
    # not), so the page reads it as it reads the gathering box's
    services.thanos.query = {
      enable = true;
      grpc-address = "127.0.0.1:10907";
      http-address = "127.0.0.1:10903";
      endpoints = [ "127.0.0.1:10901" ];
    };

    services.nginx.virtualHosts.${host} = {
      useACMEHost = base;
      forceSSL = true;
      # the page is the gathering box's; this name answers its queries
      locations."= /".return = "302 https://metrics.${base}/";
      locations."/_dd/" = {
        proxyPass = "http://127.0.0.1:${toString port}/_dd/";
        extraConfig = ''
          proxy_set_header X-Original-URI $request_uri;
        '';
      };
      locations."= /_dd/verify" = {
        proxyPass = "http://127.0.0.1:${toString port}/verify";
        extraConfig = ''
          internal;
          proxy_pass_request_body off;
          proxy_set_header Content-Length "";
          proxy_set_header X-Original-URI $request_uri;
          proxy_set_header X-Original-Method $request_method;
        '';
      };
      # Prometheus's read-only queries, and nothing else of its API. A
      # refusal is the status itself, not a redirect: the page asks from
      # metrics.<domain>, and on a 401 sends the browser through this
      # name's sign-in once and back (/_dd/back)
      locations."~ ^/api/v1/(query|query_range|series|labels|label/[^/]+/values)$" = {
        proxyPass = "http://127.0.0.1:10903";
        extraConfig = ''
          limit_except GET POST { deny all; }
          auth_request /_dd/verify;
          proxy_set_header Authorization "";
          add_header Access-Control-Allow-Origin "https://metrics.${base}" always;
          add_header Access-Control-Allow-Credentials "true" always;
          add_header Vary "Origin" always;
        '';
        ddCookie = "";
      };
    };
  };
}
