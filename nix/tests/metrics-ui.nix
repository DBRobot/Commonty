# The Metrics page and nginx's rules in front of Thanos (metrics-ui.nix):
# the page is the gate's, and only the read-only query calls reach Thanos,
# behind the sign-in check, with the caller's credential taken out. A
# stand-in on Thanos' query address writes down every request it gets, so
# the test sees what got through. Thanos itself is the thanos test's.
#
# And a joined box's metrics (joined-metrics.nix): written over the store's
# port with its own token, kept under its own name whatever it says, and
# shown to its member alone, never to the fleet's admin. Two joined boxes
# stand in here as two remote writes of this box's prometheus, each with
# its token; the agreed state is a file served where dd-control answers.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  # a certificate for the box's names, where a real box has its wildcard
  # from acme
  cert = pkgs.runCommand "test-cert" { nativeBuildInputs = [ pkgs.openssl ]; } ''
    mkdir $out
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
      -subj '/CN=test.invalid' -addext 'subjectAltName=DNS:*.test.invalid' \
      -keyout $out/key.pem -out $out/cert.pem
  '';
  # answers anything with an empty success, and logs the request line and
  # its headers
  control = pkgs.writeText "control-stand-in.py" ''
    import http.server, json

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = open("/tmp/state.json", "rb").read()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    http.server.HTTPServer(("127.0.0.1", 4195), H).serve_forever()
  '';
  thanos = pkgs.writeText "thanos-stand-in.py" ''
    import http.server

    class H(http.server.BaseHTTPRequestHandler):
        def answer(self):
            with open("/tmp/thanos.log", "a") as f:
                f.write(self.command + " " + self.path + "\n" + str(self.headers) + "\n")
            body = b'{"status":"success","data":[]}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        do_GET = do_POST = do_DELETE = do_PUT = answer

    http.server.HTTPServer(("127.0.0.1", 10903), H).serve_forever()
  '';
in
{
  name = "metrics-ui";
  helpers = true;
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1536;
  defaults.virtualisation.cores = 2;
  nodes.box =
    { config, ... }:
    {
      imports = [
        ./box.nix
        ../modules/observe/metrics-ui.nix
        ../modules/observe/joined-metrics.nix
        {
          services.nginx.virtualHosts."store.box.test.invalid" = {
            onlySSL = true;
            sslCertificate = "${cert}/cert.pem";
            sslCertificateKey = "${cert}/key.pem";
            listen = [
              {
                addr = "0.0.0.0";
                port = 3443;
                ssl = true;
              }
            ];
            locations."/_keys/".alias = "/var/lib/dd-store-sealed/";
          };
        }
      ];
      dd.joinedMetrics.receive = true;
      # this box's prometheus is both joined boxes: a's token, and b's,
      # which says it is a (its external label) and is marked as b's
      services.prometheus.globalConfig.external_labels.box = "jaaaaaaaaaa";
      services.prometheus.globalConfig.scrape_interval = "5s";
      services.prometheus.remoteWrite =
        map
          (who: {
            url = "https://store.box.test.invalid:3443/_metrics/write";
            bearer_token_file = "/var/lib/dd-metrics/${who}";
            tls_config.insecure_skip_verify = true;
            write_relabel_configs = [
              {
                target_label = "sender";
                replacement = who;
              }
            ];
            queue_config.batch_send_deadline = "2s";
          })
          [
            "a"
            "b"
          ];
      systemd.tmpfiles.rules = [
        "d /var/lib/dd-metrics 0755 root root -"
        "d /var/lib/dd-store-sealed 0755 root root -"
      ];
      networking.hosts."127.0.0.1" = [ "store.box.test.invalid" ];
      systemd.services.control-stand-in = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${control}";
      };
      dd.verify.role = lib.mkForce "full";
      # Metrics are the box owner's and the admin's alone: the member signed
      # in here is the fleet's admin
      dd.verify.admin = "sarah";
      # a tile the demo may use, so the demo exists; Metrics is never one
      dd.home.services = [
        {
          name = "Files";
          url = "https://files.test.invalid/_dd/files";
          icon = "files";
          color = "#2f6fd6";
          demo = "read";
        }
      ];
      services.nginx.enable = true;
      # the address the observe box's query answers on (thanos.nix)
      services.thanos.query.http-address = "127.0.0.1:10903";
      services.nginx.virtualHosts =
        lib.genAttrs (map (h: "${h}.test.invalid") (config.dd.verify.hosts ++ [ "grafana" ]))
          (_: {
            useACMEHost = lib.mkForce null;
            sslCertificate = "${cert}/cert.pem";
            sslCertificateKey = "${cert}/key.pem";
          });
      systemd.services.thanos-stand-in = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${thanos}";
      };
      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
        pkgs.age
      ];
    };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
