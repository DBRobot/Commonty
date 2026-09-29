{ config, lib, ... }:
{
  # A box with a public name: certificates, nginx and the browser login.
  # The verifier's full role lives here.
  imports = [
    ./_sops.nix
    ../modules/gate/acme.nix
    ../modules/gate/public.nix
    ../modules/net/headscale.nix
  ];
  # the fleet's own network is run from here
  dd.headscale.enable = true;
  # the front door on the open internet: a Cloudflare tunnel this box opens
  # outward (the house line is carrier nat; nothing can be forwarded here).
  # Off, the names point at this box's tailnet address and no outsider is
  # answered; the same unit keeps the records either way.
  dd.public = {
    enable = true;
    # the gate's own pages and the sign-up flow; modules add their own
    hosts = [
      "home"
      "accounts"
      "headscale" # the network's control server, for the app's bridge (modules/net)
    ];
    # commonty.org itself: what people type. It sends them to home and
    # offers the app download, nothing else (its server block below)
    bare = true;
    tunnel = "d0534bff-f478-48ab-a949-65e2e3c14c39";
    credentialsFile = config.sops.secrets.cloudflared-credentials.path;
    tokenFile = config.sops.templates."cloudflare.env".path;
  };
  dd.verify.role = "full";
  services.tailscale.permitCertUid = "nginx"; # so nginx can fetch *.ts.net certs without root

  sops.secrets.cloudflare-token = { };
  sops.secrets.cloudflared-credentials = { }; # systemd hands it to cloudflared as a credential
  # TMDB's read token, for posters on Movies & TV: handed to signed-in
  # pages, which look titles up from the member's own device
  sops.secrets.tmdb-token.owner = "dd-verify";
  dd.verify.tmdbKeyFile = config.sops.secrets.tmdb-token.path;
  sops.templates."cloudflare.env".content = ''
    CF_DNS_API_TOKEN=${config.sops.placeholder.cloudflare-token}
  '';

  # the bare domain: what people type to find the site, open to the web
  # through the tunnel (dd.public.bare). It sends them to home.<domain> and
  # offers the app an invited person needs before they can sign in to
  # anything; it asks for nothing and serves nothing else.
  dd.verify.appManifest = "https://git.${config.dd.domain}/${config.dd.repo}/raw/branch/releases/app.json";

  services.nginx.virtualHosts.${config.dd.domain} = {
    forceSSL = true;
    useACMEHost = config.dd.domain;
    locations."/download" = {
      proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/download";
      extraConfig = "proxy_set_header X-Original-URI $request_uri;";
    };
    locations."/_dd/static/" = {
      proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/static/";
    };
    # the front page is the download page, not a sign-in form: a new name
    # whose front page is a login is what phishing looks like to Google
    locations."= /" = {
      proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/download";
      extraConfig = "proxy_set_header X-Original-URI $request_uri;";
    };
    locations."/".return = "301 https://home.${config.dd.domain}$request_uri";
  };

  # garage's public face: ente's browser uploads and any other s3 client
  # reach the cluster through the gateway's name
  services.nginx.virtualHosts."s3.${config.dd.domain}" = {
    useACMEHost = config.dd.domain;
    forceSSL = true;
    locations."/" = {
      proxyPass = "http://127.0.0.1:3900";
      extraConfig = ''
        client_max_body_size 0;
        proxy_request_buffering off;
      '';
    };
  };
}
