# A member's remote computer on a box, the box's side: what dd-control does
# (its flag, held under the computer's lease, and systemctl) done by the
# test, and what it serves (/computers, /plan) by a stand-in on its port. The
# control plane's side - judged, placed, leased, started and stopped - is
# the control test's (control/run.py), with real validators.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  # dd-control's answers, from files the test writes
  standIn = pkgs.writeText "control-stand-in.py" ''
    import http.server

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            path = self.path.split("?")[0]
            name = {"/computers": "view.json", "/plan": "plan.json"}.get(path)
            if not name:
                self.send_error(404)
                return
            try:
                body = open(f"/var/lib/stand-in/{name}", "rb").read()
            except OSError:
                self.send_error(503)
                return
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(body)

    http.server.HTTPServer(("127.0.0.1", 4195), H).serve_forever()
  '';
  # the gate's answer of whose devices may reach a computer, as the test
  # writes it, well formed or not
  fakeGate = pkgs.writeText "fake-gate.py" ''
    import http.server

    class H(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            self.rfile.read(int(self.headers.get("content-length", 0)))
            body = open("/var/lib/stand-in/gate.json", "rb").read()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(body)

    http.server.HTTPServer(("127.0.0.1", 4181), H).serve_forever()
  '';
in
{
  name = "computers";
  # it takes about 20 minutes on a busy runner; one that hangs fails long
  # before CI's hour
  globalTimeout = 1800;
  node.specialArgs = { inherit self; };
  nodes.box = {
    imports = [
      ./box.nix
      ../modules/computers/computers.nix
    ];
    dd.computers.enable = true;
    # a small box: its 1G computer fits beside what it keeps for itself
    dd.computers.reserveMiB = 512;
    # a member's total, small enough that a second 1G computer goes over it
    dd.computers.memoryMiB = 1536;
    # the guests' filter is the firewall's (nix/modules/guests/egress.nix)
    networking.firewall.enable = lib.mkForce true;
    systemd.services.control-stand-in = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${standIn}";
    };
    systemd.tmpfiles.rules = [
      "d /run/dd-placed 0755 root root -"
      "d /var/lib/stand-in 0755 root root -"
    ];
    # the fence as box/placed.nix has it
    environment.etc."dd-placed/carries.json".text = "{}";
    systemd.services.dd-placed-fence = {
      path = [
        pkgs.jq
        pkgs.coreutils
        pkgs.systemd
      ];
      serviceConfig.Type = "oneshot";
      script = builtins.readFile ../modules/box/placed-fence.sh;
    };
    environment.etc."fake-gate.py".source = fakeGate;
    environment.systemPackages = [
      pkgs.curl
      pkgs.jq
      pkgs.openssh
      pkgs.python3
    ];
    # a 1G guest inside this one: needs a builder with kvm, nesting on
    virtualisation.memorySize = 3072;
    virtualisation.cores = 2;
    virtualisation.diskSize = 16384;
  };
}
