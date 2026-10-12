{ config, lib, ... }:
let
  mailSetup = ../../fleet/mail.json;
  dnsSetup = ../../fleet/dns-worker.json;
  idSetup = ../../fleet/id-worker.json;
  # the gateway holds a token for the zone only until the DNS Worker does
  byToken = config.dd.dnsWorker == "";
  canReadWorker = lib.hasInfix "\ncloudflare-workers-read:" (
    "\n" + builtins.readFile (../../secrets + "/${config.networking.hostName}.yaml")
  );
  # a gate path on the bare name, under the same flood limits as on home
  docsGate = {
    proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}";
    extraConfig = ''
      limit_req zone=dd_user burst=300 nodelay;
      limit_req zone=dd_visitor burst=500 nodelay;
      limit_req_status 429;
    '';
  };
in
{
  # A box with a public name: certificates, nginx and the browser login.
  # The verifier's full role lives here.
  imports = [
    ./_sops.nix
    ../modules/gate/acme.nix
    ../modules/gate/public.nix
    ../modules/gate/mail-worker.nix
    ../modules/gate/security-txt.nix
    ../modules/net/headscale.nix
  ];
  # the fleet's own network is run from here
  dd.headscale.enable = true;
  # members' addresses: kept by the mail Worker at Cloudflare; this box
  # carries its files and checks Cloudflare runs them (`dd mail setup`
  # writes fleet/mail.json; the read token is in sops once someone adds it)
  dd.mailWorker = {
    enable = true;
    account =
      if builtins.pathExists mailSetup then
        (builtins.fromJSON (builtins.readFile mailSetup)).account
      else
        "";
    dnsAccount =
      if builtins.pathExists dnsSetup then
        (builtins.fromJSON (builtins.readFile dnsSetup)).account
      else
        "";
    idAccount =
      if builtins.pathExists idSetup then (builtins.fromJSON (builtins.readFile idSetup)).account else "";
    readTokenFile =
      if canReadWorker then config.sops.templates."cloudflare-workers-read.env".path else null;
  };
  sops.secrets.cloudflare-workers-read = lib.mkIf canReadWorker { };
  sops.templates."cloudflare-workers-read.env" = lib.mkIf canReadWorker {
    content = "CF_WORKERS_READ_TOKEN=${config.sops.placeholder.cloudflare-workers-read}";
    owner = "node-exporter";
  };
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
    # The demo, for anyone: each service the demo may use answers outsiders
    # as demo-<service>, for the demo session alone. Photos is not one of
    # them: it is Ente's own app, talking to Ente's server, and neither is
    # something to open to the internet.
    demoDoors = lib.unique (
      map
        (
          t:
          lib.removeSuffix ".${config.dd.domain}" (
            lib.head (lib.splitString "/" (lib.removePrefix "https://" t.url))
          )
        )
        (lib.filter (t: t.demo != null && !(lib.hasPrefix "https://photos." t.url)) config.dd.home.services)
    );
    # commonty.org itself: what people type. The front page, the docs and
    # the app download, and home for the rest (its server block below)
    bare = true;
    tunnel = "d0534bff-f478-48ab-a949-65e2e3c14c39";
    credentialsFile = config.sops.secrets.cloudflared-credentials.path;
    # the zone's token: until the DNS Worker holds it, not after
    tokenFile = if byToken then config.sops.templates."cloudflare.env".path else null;
  };
  dd.verify.role = "full";
  dd.acme.enable = true;
  services.tailscale.permitCertUid = "nginx"; # so nginx can fetch *.ts.net certs without root

  sops.secrets.cloudflare-token = lib.mkIf byToken { };
  sops.secrets.cloudflared-credentials = { }; # systemd hands it to cloudflared as a credential
  # TMDB's read token, for posters on Movies & TV: handed to signed-in
  # pages, which look titles up from the member's own device
  sops.secrets.tmdb-token.owner = "dd-verify";
  dd.verify.tmdbKeyFile = config.sops.secrets.tmdb-token.path;
  sops.templates."cloudflare.env" = lib.mkIf byToken {
    content = ''
      CF_DNS_API_TOKEN=${config.sops.placeholder.cloudflare-token}
    '';
  };

  # the bare domain: what people type to find the site, open to the web
  # through the tunnel (dd.public.bare). It shows the front page, the docs
  # and the app an invited person needs before they can sign in to
  # anything, and sends everything else to home.<domain>; it asks for
  # nothing and serves nothing else.
  dd.verify.appManifest = "https://git.${config.dd.domain}/${config.dd.repo}/raw/branch/releases/app.json";

  services.nginx.virtualHosts.${config.dd.domain} = {
    forceSSL = true;
    useACMEHost = config.dd.domain;
    # the front page, for anyone (servers/landing); every other address
    # that is not below goes on to home
    locations."= /" = docsGate // {
      proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/landing";
    };
    # the app itself is on home, where sign-in and join already point
    locations."= /download".return = "301 https://home.${config.dd.domain}/_dd/download";
    locations."/".return = "301 https://home.${config.dd.domain}$request_uri";
    # the files the downloads page links, from the forge's release of that
    # tag: as nobody, so this name reaches only what a stranger may fetch
    # from a public repository, and only a release's files
    locations."~ ^/download/v[0-9]+\\.[0-9]+\\.[0-9]+/[A-Za-z0-9._-]+$" =
      lib.mkIf config.services.forgejo.enable
        {
          proxyPass = "http://unix:/run/forgejo/forgejo.sock:";
          extraConfig = ''
            rewrite ^/download/([^/]+)/([^/]+)$ /${config.dd.repo}/releases/download/$1/$2 break;
            proxy_set_header X-WEBAUTH-USER "";
            proxy_set_header Authorization "";
            proxy_max_temp_file_size 0;
            limit_req zone=dd_visitor burst=20 nodelay;
          '';
          # no cookie at all (a Cookie line here would be a second one: strip-cookie.nix)
          ddCookie = "";
        };
    # the docs, for anyone (servers/gate/src/docs.rs), and the few gate paths
    # their page needs: its files, the bar's who-is-looking, the index and
    # the markdown. Nothing else of the gate answers on this name.
    locations."= /docs".proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/docs/page";
    locations."/docs/".proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/docs/page/";
    locations."~ ^/_dd/(static|web|docs)/" = docsGate;
    locations."= /_dd/me" = docsGate;
    locations."= /_dd/config" = docsGate;
    locations."= /_dd/csp" = docsGate;
    # the assistant's map, for the docs' own pages: members and the demo
    # only (the gate answers 401 to anyone else), so nothing a stranger may not have
    locations."= /_dd/sitemap.json" = docsGate;
    locations."= /_dd/joke" = docsGate;
  };

  # garage's public face: ente's browser uploads and any other s3 client
  # reach the cluster through the gateway's name
  services.nginx.virtualHosts."s3.${config.dd.domain}" = {
    useACMEHost = config.dd.domain;
    forceSSL = true;
    locations."/" = {
      # the pool at this box's address for it: its own garage, or a storage
      # box's over the fleet's network when the gateway keeps none
      proxyPass = config.dd.box.s3;
      extraConfig = ''
        client_max_body_size 0;
        proxy_request_buffering off;
      '';
    };
  };
}
