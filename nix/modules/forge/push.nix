# The forge's push door: push.<domain>, for someone with a git token that a
# member made on a device of theirs (`dd git-token`, docs/dev/forge.md). Git
# over https, clone and push, and nothing else: three addresses, each asked
# of the gate first. The forge sees the member the way the Git UI does, by
# X-WEBAUTH-USER, and decides what they may write; the token never reaches it.
# A push never makes a repository: the gate says 403 for one that is not there.
{
  config,
  lib,
  options,
  ...
}:
let
  base = config.dd.domain;
  host = "push.${base}";
  sock = "/run/forgejo/forgejo.sock";
  gate = "http://127.0.0.1:${toString config.dd.verify.port}";
  # /<owner>/<repo>[.git]: plain characters only, so what nginx matched is
  # what the gate reads in the token's repository
  repo = "/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+";
  # a 401 is git's cue to send the token it was given
  challenge = ''
    error_page 401 = @challenge;
  '';
  # the forge, as whoever the gate says the token is
  toForge = extra: {
    proxyPass = "http://unix:${sock}:";
    ddCookie = "";
    ddCsp = "default-src 'none'; sandbox";
    extraConfig = ''
      auth_request /_dd/git-verify;
      auth_request_set $auth_user $upstream_http_x_auth_request_preferred_username;
      proxy_set_header X-WEBAUTH-USER $forge_user;
      proxy_set_header X-WEBAUTH-EMAIL "";
      proxy_set_header X-WEBAUTH-FULLNAME "";
      proxy_set_header Authorization "";
      proxy_set_header Sudo "";
      proxy_set_header X-Gitea-OTP "";
      proxy_set_header X-Forgejo-OTP "";
      proxy_hide_header Set-Cookie;
      proxy_hide_header WWW-Authenticate;
      proxy_max_temp_file_size 0;
      # the public door's own zones (door.nix): a few at a time from anyone,
      # a handful from everyone together, each one bounded
      limit_req zone=dd_code_git burst=20 nodelay;
      limit_req_status 429;
      limit_conn dd_code_addr 2;
      limit_conn dd_code_all 8;
      limit_conn_status 429;
      proxy_read_timeout 60s;
      proxy_send_timeout 60s;
      send_timeout 60s;
    ''
    + challenge
    + extra;
  };
in
{
  config = lib.mkMerge [
    (lib.optionalAttrs (options.dd ? public) {
      dd.public.hosts = [ "push" ];
    })
    {
      # tokens are for this name: its audience, as the gate counts names
      dd.verify.names = [ host ];
      services.nginx.virtualHosts.${host} = {
        useACMEHost = base;
        forceSSL = true;
        extraConfig = ''
          # a token or sudo in the query would make the forge someone, a name it
          # decodes could be either, and nothing here needs a percent-escape
          if ($args ~* "(^|[&;])(token|access_token|sudo)=") { return 403; }
          if ($request_uri ~ "%") { return 400; }
          if ($request_uri ~* "(/|^)\.\.?(/|\?|$)") { return 400; }
        '';
        locations = {
          "~ ^${repo}/info/refs$" = toForge ''
            if ($request_method !~ ^(GET|HEAD)$) { return 405; }
            if ($arg_service !~ ^git-(upload|receive)-pack$) { return 403; }
          '';
          "~ ^${repo}/git-upload-pack$" = toForge ''
            if ($request_method != POST) { return 405; }
            client_max_body_size 4m;
            client_body_timeout 15s;
          '';
          # 90m only because Cloudflare turns away a body over 100 MB (free
          # plan) before it reaches us: under that, the refusal is ours
          "~ ^${repo}/git-receive-pack$" = toForge ''
            if ($request_method != POST) { return 405; }
            client_max_body_size 90m;
            client_body_timeout 60s;
          '';
          "= /_dd/git-verify" = {
            proxyPass = "${gate}/git-verify";
            extraConfig = ''
              internal;
              proxy_pass_request_body off;
              proxy_set_header Content-Length "";
              proxy_set_header X-Original-URI $request_uri;
              client_max_body_size 0;
            '';
          };
          "@challenge".extraConfig = ''
            add_header WWW-Authenticate 'Basic realm="commonty git"' always;
            return 401;
          '';
          "/".extraConfig = ''
            return 404;
          '';
        };
      };
    }
  ];
}
