{ config, ... }:
let
  base = config.dd.domain;
  query = "http://${config.services.thanos.query.http-address}";
in
{
  # Metrics: our own page (box/observe/web) over Thanos, which answers for
  # every box's Prometheus. The page asks /api/v1 as whoever the gate says
  # is looking, and only these read-only calls go through: no admin API,
  # nothing that writes.
  services.nginx.virtualHosts."metrics.${base}" = {
    useACMEHost = base;
    forceSSL = true;
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}";
      extraConfig = ''
        rewrite ^ /_dd/metrics break;
        proxy_set_header X-Original-URI $request_uri;
      '';
    };
    locations."~ ^/api/v1/(query|query_range|series|labels|label/[^/]+/values)$" = {
      proxyPass = query;
      extraConfig = ''
        limit_except GET POST { deny all; }
        auth_request /_dd/verify;
        error_page 401 = @login;
        error_page 403 = @waiting;
        proxy_set_header Authorization "";
        proxy_set_header Cookie "";
      '';
    };
  };
  # where Metrics used to be: links and bookmarks still arrive
  services.nginx.virtualHosts."grafana.${base}" = {
    useACMEHost = base;
    forceSSL = true;
    locations."/".return = "301 https://metrics.${base}$request_uri";
  };
}
