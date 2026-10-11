# The Metrics page and nginx's rules in front of Thanos (metrics-ui.nix):
# the page is the gate's, and only the read-only query calls reach Thanos,
# behind the sign-in check, with the caller's credential taken out. A
# stand-in on Thanos' query address writes down every request it gets, so
# the test sees what got through. Thanos itself is the thanos test's.
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
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1536;
  defaults.virtualisation.cores = 2;
  nodes.box =
    { config, ... }:
    {
      imports = [
        ./box.nix
        ../modules/observe/metrics-ui.nix
      ];
      dd.verify.role = lib.mkForce "full";
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
      ];
    };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
