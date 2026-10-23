# Each web service's one main thing, end to end, in a browser on one box
# with the real services behind it (checks.site-journeys): sign in, open
# the service, do it, check what came of it, sign out. The box runs the
# gateway's own modules - the gate and nginx, its store and the libraries,
# Garage for museum (with failover off, as photos.nix),
# Forgejo, the games manager, Prometheus behind a Thanos query - and a
# model that answers one fixed line where llama-swap would be, behind
# nginx's real path to it. A member's browser reaches the pages that hold
# a key through their app (app/src/local.rs): here a stand-in for the
# app's few lines of routing (site/app.py), with the box real behind it.
#
# The scenario: site-journeys.py on the driver, site/journeys.py in the
# browser on the box.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  system = pkgs.stdenv.hostPlatform.system;
  domain = "test.invalid";
  rpc = pkgs.writeText "garage.env" "GARAGE_RPC_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
  cert = pkgs.runCommand "test-cert" { nativeBuildInputs = [ pkgs.openssl ]; } ''
    mkdir $out
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
      -subj '/CN=${domain}' -addext 'subjectAltName=DNS:*.${domain},DNS:${domain}' \
      -keyout $out/key.pem -out $out/cert.pem
  '';
  python = pkgs.python3.withPackages (ps: [ ps.playwright ]);
  # a real picture for the demo's gallery, which shows only what a browser can
  picture = pkgs.runCommand "otter.jpg" { nativeBuildInputs = [ pkgs.imagemagick ]; } ''
    magick -size 64x48 gradient:skyblue-navy jpeg:$out
  '';
  # Photos' secrets, test values (photos.nix): the fleet's code, museum's
  # keys, the bucket's key, and the demo's password, which the gate holds
  photoSecrets = pkgs.runCommand "photos-test-secrets" { } ''
    mkdir $out
    echo 123456 > $out/ente-ott
    head -c 32 /dev/zero | base64 -w0 > $out/ente-key-encryption
    head -c 64 /dev/zero | base64 -w0 > $out/ente-key-hash
    head -c 32 /dev/zero | base64 -w0 > $out/ente-jwt-secret
    echo GK0123456789abcdef01234568 > $out/garage-key-id
    echo 1111111111111111111111111111111111111111111111111111111111111111 > $out/garage-key-secret
    echo a-long-demo-password > $out/ente-demo-password
  '';
  # the model: one fixed answer, in the stream the Chat page reads, and only
  # for the secret nginx was given at boot (llm.nix proxy.conf)
  model = pkgs.writeText "model-stand-in.py" ''
    import json, os, socketserver, time
    from http.server import BaseHTTPRequestHandler

    def key():
        for line in open("/run/dd-llm/env"):
            if line.startswith("LLAMA_API_KEY="):
                return line.split("=", 1)[1].strip()

    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        def address_string(self):
            return "model"
        def send(self, code, ctype, body):
            self.send_response(code)
            self.send_header("content-type", ctype)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def go(self):
            n = int(self.headers.get("content-length") or 0)
            asked = self.rfile.read(n) if n else b""
            with open("/tmp/model.log", "a") as f:
                f.write(json.dumps({"path": self.path, "auth": self.headers.get("authorization") == "Bearer " + key(), "cookie": self.headers.get("cookie"), "body": asked.decode(errors="replace")}) + "\n")
            if self.headers.get("authorization") != "Bearer " + key():
                return self.send(401, "text/plain", b"no")
            if self.path.startswith("/v1/models"):
                # late, as from a box busy waking a model: the Chat journey
                # sends before the page has the list
                time.sleep(3)
                return self.send(200, "application/json", json.dumps({"object": "list", "data": [{"id": "stand-in", "object": "model"}]}).encode())
            if self.path == "/running":
                return self.send(200, "application/json", b'{"running": []}')
            if self.path.startswith("/v1/chat/completions"):
                words = ["Otters ", "hold ", "hands."]
                out = b""
                for w in words:
                    out += b"data: " + json.dumps({"choices": [{"index": 0, "delta": {"content": w}}]}).encode() + b"\n\n"
                out += b"data: [DONE]\n\n"
                return self.send(200, "text/event-stream", out)
            self.send(404, "text/plain", b"no")
        do_GET = do_POST = go

    class S(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
        daemon_threads = True

    p = "/run/dd-llm/llm.sock"
    if os.path.exists(p):
        os.unlink(p)
    s = S(p, H)
    os.chmod(p, 0o660)
    os.chown(p, -1, int(os.environ["NGINX_GID"]))
    s.serve_forever()
  '';
in
{
  name = "site-journeys";
  helpers = true;
  node.specialArgs = { inherit self; };
  # ente.nix adds an overlay (photos.nix does the same)
  node.pkgsReadOnly = false;
  nodes.box =
    { config, ... }:
    {
      imports = [
        ./box.nix
        ../modules/storage/garage.nix
        ../modules/storage/store.nix
        ../modules/library/libraries.nix
        ../modules/forge/forgejo.nix
        ../modules/games/games.nix
        ../modules/llm/llm.nix
        ../modules/observe/metrics-ui.nix
        ../modules/photos/ente.nix
        # the secrets a box has from sops, here from the store
        {
          options.sops.secrets = lib.mkOption {
            type = lib.types.attrsOf (
              lib.types.submodule (
                { name, ... }:
                {
                  options.path = lib.mkOption {
                    type = lib.types.str;
                    default = "${photoSecrets}/${name}";
                  };
                }
              )
            );
          };
        }
      ];
      sops.secrets = lib.genAttrs [
        "ente-ott"
        "ente-key-encryption"
        "ente-key-hash"
        "ente-jwt-secret"
        "garage-key-id"
        "garage-key-secret"
      ] (_: { });
      # Photos: museum on this box, and the demo's gallery the gate shows
      dd.verify.photos = {
        api = "http://127.0.0.1:8080";
        emailSuffix = "@users.${domain}";
        codeFile = "${photoSecrets}/ente-ott";
        demoPasswordFile = "${photoSecrets}/ente-demo-password";
      };
      services.ente.web.enable = lib.mkForce false;
      services.ente.api.settings = {
        s3 = {
          are_local_buckets = true;
          b2-eu-cen.endpoint = lib.mkForce "127.0.0.1:3900";
        };
        apps = {
          accounts = "https://accounts.${domain}";
          cast = "https://cast.${domain}";
          public-albums = "https://albums.${domain}";
        };
        webauthn = {
          rpid = "accounts.${domain}";
          rporigins = [ "https://accounts.${domain}" ];
        };
      };
      dd.domain = lib.mkForce domain;
      dd.verify.role = lib.mkForce "full";
      # the member signed in here owns this box and is the fleet's admin:
      # Metrics is theirs to open
      dd.verify.admin = "mia";
      dd.box.owner = "mia";
      dd.verify.boxes.box = {
        owner = "mia";
        metrics = "";
      };
      dd.garage = {
        zone = "r-test";
        capacity = "1G";
        dataDir = "/srv/garage";
        publicAddr = "box:3901";
        envFile = rpc;
        replicationFactor = 1;
      };
      # the libraries in the box's own store, which it writes (as library.nix)
      dd.store = {
        enable = true;
        capacity = "1G";
        agePlugins = [ ];
        buckets.libraries.writer = "box";
        buckets.mail.writer = "box";
      };
      dd.libraries.enable = true;
      systemd.services.zfs-datasets = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
        };
        script = "mkdir -p /vault";
      };
      dd.games.enable = true;
      # the games' guests are kept off the box's loopback by its firewall
      networking.firewall.enable = lib.mkForce true;
      dd.games.catalogue = pkgs.writeText "catalogue.json" (
        builtins.toJSON {
          games = [
            {
              id = "probe";
              name = "Probe";
              description = "a web server standing in for a game";
              app = 1;
              game = null;
              cover = false;
              wine = false;
              startup = "python3 -u -m http.server {{SERVER_PORT}}";
              ready = "Serving HTTP";
              stop = "^C";
              files = { };
              install = "true";
              settings = [ ];
              ports = [ "SERVER_PORT" ];
              memory_mib = 512;
            }
          ];
        }
      );
      dd.games.covers = pkgs.emptyDirectory;
      dd.games.portBase = 27015;
      dd.games.memoryMiB = 512;
      # the figures: this box's Prometheus, read through a Thanos query as
      # the observe box's are; its own name on every series
      services.prometheus.globalConfig = {
        scrape_interval = "5s";
        external_labels.box = "box";
      };
      services.thanos.sidecar = {
        enable = true;
        prometheus.url = "http://127.0.0.1:9090";
        grpc-address = "127.0.0.1:10901";
        http-address = "127.0.0.1:10902";
      };
      services.thanos.query = {
        enable = true;
        endpoints = [ "127.0.0.1:10901" ];
        grpc-address = "127.0.0.1:10904";
        http-address = "127.0.0.1:10903";
      };
      # the tiles, as the roles have them: a demo allowance makes the demo
      dd.home.services = [
        {
          name = "Files";
          url = "https://files.${domain}/_dd/files";
          icon = "files";
          color = "#2f6fd6";
          demo = "read";
        }
        {
          name = "Code";
          url = "https://git.${domain}/";
          icon = "code";
          color = "#4a5a8a";
        }
        {
          name = "Games";
          url = "https://games.${domain}/";
          icon = "games";
          color = "#3b8a4a";
        }
        {
          name = "Chat";
          url = "https://files.${domain}/_dd/chat";
          icon = "chat";
          color = "#7a4ab0";
        }
      ];
      systemd.services.model-stand-in = {
        wantedBy = [ "multi-user.target" ];
        after = [ "dd-llm-key.service" ];
        requires = [ "dd-llm-key.service" ];
        environment.NGINX_GID = toString config.ids.gids.nginx;
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${model}";
      };
      services.nginx.enable = true;
      services.nginx.virtualHosts = lib.mkMerge [
        (lib.genAttrs
          (map (h: "${h}.${domain}") (
            lib.unique (
              config.dd.verify.hosts
              ++ [
                "code"
                "grafana"
                "api"
                "accounts"
                "albums"
                "cast"
                "locker"
              ]
            )
          ))
          (_: {
            useACMEHost = lib.mkForce null;
            # https on every name, as the gateway's roles have it (gate/public.nix)
            forceSSL = true;
            sslCertificate = "${cert}/cert.pem";
            sslCertificateKey = "${cert}/key.pem";
          })
        )
        # Locker is one of Ente's web apps left out (photos.nix)
        { "locker.${domain}".locations."/".root = lib.mkForce "/var/empty"; }
        # the store's: no certificate in a test, and no other box reaches it
        {
          "store.box.${domain}" = {
            useACMEHost = lib.mkForce null;
            onlySSL = lib.mkForce false;
            listen = lib.mkForce [
              {
                addr = "127.0.0.1";
                port = 3443;
              }
            ];
          };
        }
      ];
      networking.hosts."127.0.0.1" = map (h: "${h}.${domain}") config.dd.verify.hosts;
      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
        python
      ];
      environment.variables.PLAYWRIGHT_BROWSERS_PATH = "${pkgs.playwright-driver.browsers}";
      virtualisation.memorySize = 5120;
      virtualisation.cores = 4;
      virtualisation.diskSize = 8192;
    };
  scriptEnv = {
    dd = "${self.packages.${system}.dd}/bin/dd";
    python = "${python}/bin/python3";
    browsers = "${pkgs.playwright-driver.browsers}";
    fonts = "${pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; }}";
    journeys = "${./site/journeys.py}";
    app = "${./site/app.py}";
    carried = "${../../app/web/_dd}";
    web = "${self.packages.${system}.web}";
    ca = "${cert}/cert.pem";
    roundtrip = "${self.packages.${system}.ente-roundtrip}/bin/ente-roundtrip";
    picture = "${picture}";
    inherit domain;
  };
}
