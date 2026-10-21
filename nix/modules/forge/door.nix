# The forge's public door: code.<domain>, for anyone, signed in as nobody.
# Every public repository can be read and cloned there, and nothing else
# can be done: no sign-in, no write, no API but the reads the Git pages make.
#
# A name of its own rather than git.<domain> for strangers: git.<domain> is
# the members' forge, on the fleet's network, where nginx names the person
# to Forgejo. Opening it to the tunnel would put members' sessions through
# Cloudflare and leave one server block deciding per request who is whom.
# Here no request is anyone, whatever it carries.
{
  config,
  lib,
  options,
  ...
}:
let
  base = config.dd.domain;
  host = "code.${base}";
  sock = "/run/forgejo/forgejo.sock";
  gate = "http://127.0.0.1:${toString config.dd.verify.port}";
  # first path segments that are the forge's or ours, never an owner
  reserved = lib.concatStringsSep "|" [
    "api"
    "assets"
    "avatars"
    "repo-avatars"
    "attachments"
    "user"
    "login"
    "-"
    "\\.well-known"
    "_dd"
    "new"
    "search"
    "settings"
    "notifications"
    "explore"
    "admin"
    "org"
    "repo"
    "favicon\\.ico"
    "robots\\.txt"
    "fleet-repo\\.json"
  ];
  # /<owner>/<repo>, never one of the forge's own addresses
  repo = "/(?!(${reserved})/)[^/]+/[^/]+";
  # what the Git pages read, and only that (servers/forge/web, the views
  # route() marks for anyone): every one a GET
  # (an owner is never one of the forge's own words under /repos/, or
  # /repos/issues/search would pass for a repository)
  r = "repos/(?!(search|issues|migrate)/)[^/]+/[^/]+";
  api = lib.concatStringsSep "|" [
    "repos/search"
    "users/[^/]+/repos"
    r
    "${r}/(branches|tags|languages|commits|releases)"
    "${r}/contents(/.*)?"
    "${r}/raw/.+"
    "${r}/git/commits/[0-9a-f]+(\\.diff)?"
    "${r}/git/trees/.+"
    "${r}/commits/.+/status"
    "${r}/compare/.+"
    "${r}/releases/tags/.+"
  ];
  # the page and what it loads: the gate's files, our scripts alone
  csp = lib.concatStringsSep "; " [
    "default-src 'none'"
    "script-src 'self'"
    "style-src 'self' 'unsafe-inline'"
    "img-src 'self' data:"
    "font-src 'self' data:"
    "connect-src 'self'"
    "base-uri 'none'"
    "form-action 'self'"
    "frame-ancestors 'none'"
  ];
  # what the forge answers is data, a file of someone's repository among
  # it: opened on its own, nothing in it runs here
  fileCsp = "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox";
  readOnly = ''
    if ($request_method !~ ^(GET|HEAD)$) { return 405; }
  '';
  # the forge, as nobody. Every header it would take a person from is
  # emptied, whatever the request sent; its cookies go neither way; and a
  # 401 or 403 (a private repository's answer to a stranger) is a 404, so
  # one is never told apart from one that does not exist.
  nobody = ''
    proxy_set_header X-WEBAUTH-USER "";
    proxy_set_header X-WEBAUTH-EMAIL "";
    proxy_set_header X-WEBAUTH-FULLNAME "";
    proxy_set_header Authorization "";
    proxy_set_header Sudo "";
    proxy_set_header X-Gitea-OTP "";
    proxy_set_header X-Forgejo-OTP "";
    proxy_hide_header Set-Cookie;
  '';
  toForge = extra: {
    proxyPass = "http://unix:${sock}:";
    ddCookie = "";
    ddCsp = fileCsp;
    extraConfig =
      nobody
      + ''
        proxy_hide_header WWW-Authenticate;
        proxy_intercept_errors on;
        error_page 401 403 =404 /_dd/none;
      ''
      + extra;
  };
  # pages, the API's reads, files: a person's pace, and a page of a
  # repository asks for a dozen things at once
  paced = ''
    limit_req zone=dd_code burst=100 nodelay;
    limit_req_status 429;
    limit_conn dd_code_addr 32;
    limit_conn_status 429;
  '';
  # what makes the forge pack or compress a repository: a few at a time
  # from anyone, a handful from everyone together, and each one bounded
  heavy = zone: burst: ''
    limit_req zone=${zone} burst=${toString burst} nodelay;
    limit_req_status 429;
    limit_conn dd_code_addr 2;
    limit_conn dd_code_all 8;
    limit_conn_status 429;
    proxy_read_timeout 60s;
    proxy_send_timeout 60s;
    send_timeout 60s;
    proxy_max_temp_file_size 0;
    limit_rate 4m;
  '';
  toGate = {
    proxyPass = gate;
    ddCsp = csp;
    extraConfig =
      readOnly
      + paced
      + nobody
      + ''
        proxy_set_header Cookie "";
      '';
  };
  page = toGate // {
    extraConfig = toGate.extraConfig + ''
      rewrite ^ /_dd/git/public break;
    '';
  };
in
{
  config = lib.mkMerge [
    # outsiders reach it through the tunnel (modules/gate/public.nix): the
    # flood limits of every public name, and these below
    (lib.optionalAttrs (options.dd ? public) {
      dd.public.hosts = [ "code" ];
    })
    {
      services.nginx.appendHttpConfig = ''
        limit_req_zone $binary_remote_addr zone=dd_code:2m rate=20r/s;
        limit_req_zone $binary_remote_addr zone=dd_code_git:2m rate=2r/s;
        limit_req_zone $binary_remote_addr zone=dd_code_archive:1m rate=6r/m;
        limit_conn_zone $binary_remote_addr zone=dd_code_addr:2m;
        limit_conn_zone $server_name zone=dd_code_all:1m;
      '';
      services.nginx.virtualHosts.${host} = {
        useACMEHost = base;
        forceSSL = true;
        extraConfig = ''
          # a token or sudo in the query would make the forge someone, and a
          # name it decodes could be either; nothing here is anyone
          if ($args ~* "(^|[&;])(token|access_token|sudo)=") { return 403; }
          if ($args ~ "(^|[&;])[^=&;]*%") { return 403; }
          # no dot segments, plain or encoded: what nginx matched is what
          # the forge is asked
          if ($request_uri ~* "(/|%2f)(\.|%2e)(\.|%2e)?(/|%2f|\?|$)") { return 400; }
        '';
        locations = {
          # the Git pages, the views anyone may see (route(..., true))
          "= /" = page;
          "~ ^/(?!(${reserved})(/|$))[^/]+/?$" = page;
          "~ ^${repo}(/(tree|blob|commits|src/(branch|tag|commit))/.+|/commit/[0-9a-f]+|/branches|/releases|/commits|/releases/tag/[^/]+)?/?$" =
            page;
          "~ ^/_dd/(static|web)/[^/]+$" = toGate;
          # nobody is signed in here: the bar says so, and offers the way in
          "= /_dd/me".extraConfig = ''
            return 401;
          '';
          "~ ^/api/v1/(${api})$" = toForge (readOnly + paced);
          "~ ^${repo}/(raw|media|releases/download)/." = toForge (readOnly + paced);
          "~ ^/(avatars|repo-avatars)/[^/]+$" = toForge (readOnly + paced);
          "~ ^/assets/img/[^/]+$" = toForge (readOnly + paced);
          "~ ^${repo}/archive/[^/]+$" = toForge (readOnly + heavy "dd_code_archive" 3);
          # clone and fetch, and never push: the refs for upload-pack only,
          # and upload-pack itself, whose request is all a POST may be here
          "~ ^${repo}/info/refs$" = toForge (
            readOnly
            + ''
              if ($arg_service != "git-upload-pack") { return 403; }
            ''
            + heavy "dd_code_git" 20
          );
          "~ ^${repo}/git-upload-pack$" = toForge (
            ''
              if ($request_method != POST) { return 405; }
              client_max_body_size 4m;
              client_body_timeout 15s;
            ''
            + heavy "dd_code_git" 20
          );
          # crawlers index an owner's page and a repository's front page;
          # every file, commit and diff below one is thousands of pages for
          # the house line to serve. The page's own scripts stay open (the
          # longer rule wins), or a crawler could not draw it
          "= /robots.txt".extraConfig = ''
            default_type text/plain;
            return 200 "User-agent: *\nAllow: /_dd/\nDisallow: /api/\nDisallow: /*/*/\nDisallow: /*?\n";
          '';
          # what a 401 or 403 from the forge becomes. nginx carries the
          # forge's WWW-Authenticate into an intercepted 401, which would say
          # "this exists, sign in"; an access check that passes under
          # `satisfy any` drops it (ngx_http_core_access_phase), so the 404
          # comes after that phase, not from a `return`
          "= /_dd/none".extraConfig = ''
            internal;
            satisfy any;
            allow all;
            try_files /nothing =404;
          '';
          # everything else: the forge's own pages, sign-in, settings, admin,
          # notifications, packages, LFS, the runners' API, every other /api
          # and /_dd path, git-receive-pack
          "/".extraConfig = ''
            return 404;
          '';
        };
      };
    }
  ];
}
