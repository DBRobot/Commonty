# A switch that other machines cannot hold up (2026-10-22, node2 to release
# 358): the forge, on another box, is down, and the box's certificate order
# hangs on an ACME server that never answers. The switch completes, is not
# rolled back, and the box still learns each release, from the relay's copy
# while the forge is away and from the forge once it is back, handing the
# relay what it lacks. The forge's machine stands in for the relay too, on
# another port, and for an ACME server that accepts and never answers.
{ pkgs, self, ... }:
let
  dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  # the relay's /release, as the Worker keeps it, without its checks: the
  # agent's own are what is under test
  relay = pkgs.writeText "relay.py" ''
    import http.server, os
    F = "/srv/relay/release"
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != "/release" or not os.path.exists(F):
                return self.send_error(404)
            b = open(F, "rb").read()
            self.send_response(200)
            self.send_header("content-length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)
        def do_PUT(self):
            b = self.rfile.read(int(self.headers["content-length"]))
            open(F, "wb").write(b)
            self.send_response(201)
            self.send_header("content-length", "0")
            self.end_headers()
    os.makedirs("/srv/relay", exist_ok=True)
    http.server.ThreadingHTTPServer(("", 8080), H).serve_forever()
  '';
  # an ACME server that takes the connection and the TLS handshake, and
  # then never answers (lego requires https, and waits on it for ever)
  cert = import ../lib/test-cert.nix { inherit pkgs; } {
    subject = "forge";
    names = [ "forge" ];
    name = "forge-cert";
  };
  blackhole = pkgs.writeText "blackhole.py" ''
    import socket, ssl, threading
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain("${cert}/cert.pem", "${cert}/key.pem")
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("", 14000))
    s.listen(16)
    held = []
    def hold(c):
        try:
            held.append(ctx.wrap_socket(c, server_side=True))
        except Exception:
            c.close()
    while True:
        c, _ = s.accept()
        threading.Thread(target=hold, args=(c,), daemon=True).start()
  '';
in
{
  name = "switch-remote";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  defaults.virtualisation.diskSize = 4096;
  nodes = {
    forge = {
      services.nginx = {
        enable = true;
        virtualHosts.forge.root = "/srv/release";
      };
      systemd.services.relay = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${relay}";
      };
      systemd.services.blackhole = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${blackhole}";
      };
      networking.firewall.enable = false;
      environment.systemPackages = [
        pkgs.git
        pkgs.curl
        pkgs.iptables
      ];
    };
    box =
      { config, lib, ... }:
      let
        base = config.dd.domain;
      in
      {
        imports = [
          ./box.nix
          ../modules/box/agent.nix
          ../modules/gate/acme.nix
          ../modules/forge/forgejo-runner.nix
          { options.sops = lib.mkOption { type = lib.types.anything; }; }
        ];
        services.openssh.enable = true;
        dd.agent = {
          enable = true;
          url = "http://forge/current.json";
          mirrors = [ "http://forge:8080/release" ];
          publicKeyFile = "/root/release.pub";
          probeSeconds = 30;
          # the relay's port answers while the forge's does not
          reach = [ "forge:8080" ];
        };

        # the certificate, ordered of an ACME server that never answers
        dd.acme.enable = true;
        dd.acme.worker = "http://forge:8787";
        security.acme.defaults.server = "https://forge:14000/dir";
        security.pki.certificateFiles = [ "${cert}/cert.pem" ];
        services.nginx.virtualHosts."home.${base}" = {
          useACMEHost = base;
          onlySSL = true;
        };

        # the host runner, whose forge (git.test.invalid) is nowhere. Not
        # started at boot: the switch to the next release starts it
        sops.placeholder.forgejo-runner-token = "unused";
        sops.templates."forgejo-runner.env".path = "/run/runner.env";
        systemd.tmpfiles.rules = [ "f /run/runner.env 0600 root root - TOKEN=unused" ];
        dd.runner.capacity = 1;
        systemd.services.gitea-runner-box.wantedBy = lib.mkForce [ ];

        # the next release: a name more on the certificate (so its order is
        # a new one), and the runner wanted
        specialisation.next.configuration = {
          environment.etc."dd-marker".text = "next\n";
          services.nginx.virtualHosts."files.${base}" = {
            useACMEHost = base;
            onlySSL = true;
          };
          systemd.services.gitea-runner-box.wantedBy = lib.mkOverride 10 [ "multi-user.target" ];
        };
        # and the one after (a switch back to the base system would install
        # its boot loader, which a test VM has none of)
        specialisation.later.configuration.environment.etc."dd-marker".text = "later\n";
      };
  };
  scriptEnv =
    { nodes, ... }:
    {
      inherit dd;
      base = "${nodes.box.system.build.toplevel}";
      next = "${nodes.box.specialisation.next.configuration.system.build.toplevel}";
      later = "${nodes.box.specialisation.later.configuration.system.build.toplevel}";
      order = "acme-order-renew-${nodes.box.dd.domain}.service";
      cert = "acme-${nodes.box.dd.domain}.service";
    };
}
