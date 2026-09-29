{ config, pkgs, ... }:
let
  base = config.dd.domain;
  host = "llm.${base}";
  # nginx asks the verifier who is calling; the model servers have no idea.
  # This box runs CI jobs and game guests, and both reach loopback, so "on
  # this machine" is not an identity here. The model servers live in a
  # network of their own (llama-cpp.nix) that nginx reaches through a socket
  # only it may open, and llama-swap still wants a secret made at boot that
  # nginx sends with every request.
  run = "/run/dd-llm";
in
{
  # reads plaintext: only on a box whose owner is trusted with it (modules/box.nix)
  dd.box.plaintext = [ "llm (prompts and answers)" ];
  # The gateway in front of llama-server: nginx asks the verifier who this is
  # (a device-signed token, or a passkey session on this box) and proxies.
  # Nothing here can mint a token.
  services.nginx.virtualHosts.${host} = {
    useACMEHost = base;
    forceSSL = true;

    # the page: the gate's (box/chat), which asks who is looking itself
    locations."= /" = {
      proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/chat";
      extraConfig = "proxy_set_header X-Original-URI $request_uri;";
    };
    # web search, for members: the gate checks who is asking and asks
    # SearXNG on this box (search.nix); the model never touches the network
    locations."= /search" = {
      proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/chat/search";
      # a search goes out to the web for a message: paced like one
      extraConfig = ''
        proxy_set_header X-Original-URI $request_uri;
        limit_req zone=dd_model burst=20 nodelay;
        limit_req_status 429;
      '';
    };
    # which model is awake, for the picker: behind the same gate
    locations."= /running" = {
      proxyPass = "http://unix:${run}/llm.sock:/running";
      extraConfig = ''
        auth_request /_dd/verify;
        include ${run}/proxy.conf;
        error_page 401 = @login;
        error_page 403 = @waiting;
      '';
    };
    # The OpenAI-shaped API, for the page and for any client with a device
    # token. Only this: llama-swap's own pages, logs and raw upstream access
    # are not reachable from outside.
    locations."/v1/" = {
      proxyPass = "http://unix:${run}/llm.sock";
      extraConfig = ''
        auth_request /_dd/verify;
        # the caller's own bearer never reaches llama-server: it is replaced
        # by the one llama-server was started with
        include ${run}/proxy.conf;
        # a browser with no credential gets the box's passkey page (@login is
        # defined for every browser-facing vhost in modules/verify.nix)
        error_page 401 = @login;
        error_page 403 = @waiting;
        # the demo's prompts are counted at the gate (rate:N on the tile);
        # everyone's, here, against a flood (modules/gate/verify.nix)
        limit_req zone=dd_model burst=20 nodelay;
        limit_req_status 429;
        proxy_buffering off; # streamed completions
        proxy_read_timeout 600s; # cpu generation is slow
        client_max_body_size 0;
      '';
    };
  };

  # the secret, before either side that needs it. llama-swap runs under a
  # DynamicUser, so the key reaches it as an environment file systemd reads
  # as root rather than as a file it would have to be given a group for.
  systemd.services.dd-llm-key = {
    description = "the key nginx proves itself to llama-server with";
    wantedBy = [ "multi-user.target" ];
    before = [
      "nginx.service"
      "llama-swap.service"
    ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      UMask = "0077";
    };
    script = ''
      install -d -m 0751 ${run}
      key=$(${pkgs.openssl}/bin/openssl rand -hex 32)
      printf 'LLAMA_API_KEY=%s\n' "$key" > ${run}/env
      printf 'proxy_set_header Authorization "Bearer %s";\n' "$key" > ${run}/proxy.conf
      chgrp nginx ${run}/proxy.conf && chmod 0640 ${run}/proxy.conf
    '';
  };
  systemd.services.nginx.after = [ "dd-llm-key.service" ];
  # nginx refuses to start on an include it cannot open, and nginx is the
  # whole box's front door. The file exists from boot whatever the key unit
  # does, so a failure here costs the llm and nothing else.
  systemd.tmpfiles.rules = [
    "d ${run} 0751 root root -"
    "f ${run}/proxy.conf 0640 root nginx -"
  ];
}
