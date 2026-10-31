# A box that joined, shut down cleanly by its power button: what it runs is
# handed on first, and a shutdown is never held past its bound. A stand-in
# answers as dd-control does, moving the flags as a plan would. Not on the
# release's boxes: asserted of node1, whose system this test evaluates.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  node1 = self.nixosConfigurations.node1.config;
  seconds = 12;
  standin = pkgs.writeText "standin.py" ''
    import json, os, pathlib
    from http.server import BaseHTTPRequestHandler, HTTPServer

    mode = pathlib.Path("/var/lib/standin/mode").read_text().strip()
    log = open("/var/lib/standin/log", "a", buffering=1)
    flag = pathlib.Path("/run/dd-placed/svc")
    drained = pathlib.Path("/var/lib/dd-facts/draining.prom")


    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            leaving = drained.exists()
            if leaving and mode == "moves" and flag.exists():
                log.write("moved\n")
                flag.unlink()
            facts = {"draining": 1} if leaving else {}
            kept = [{"service": "svc"}] if mode == "stays" else []
            name = os.uname().nodename
            body = {"/state": {"boxes": {name: {"facts": facts}}}, "/plan": {"boxes": {name: kept}}}
            self.send_response(200)
            self.end_headers()
            self.wfile.write(json.dumps(body.get(self.path, {})).encode())


    HTTPServer(("127.0.0.1", 4195), H).serve_forever()
  '';
in
assert !(node1.systemd.services ? dd-shutdown-drain);
assert node1.services.logind.settings.Login.HandlePowerKey == "ignore";
{
  name = "shutdown-drain";
  helpers = true;
  scriptEnv = {
    inherit seconds;
  };
  nodes.box =
    { lib, ... }:
    {
      imports = [ ../modules/box/shutdown-drain.nix ];
      options.dd.box.joined = lib.mkOption { default = true; };
      config = {
        dd.shutdownDrain.seconds = seconds;
        environment.etc."dd-placed/carries.json".text = builtins.toJSON { svc = [ "served.service" ]; };
        systemd.tmpfiles.rules = [
          "d /run/dd-placed 0755 root root -"
          "d /var/lib/standin 0755 root root -"
        ];
        # what the box serves: alive until the drain lets go, and its stop
        # written where the next boot reads it
        systemd.services.served = {
          wantedBy = [ "multi-user.target" ];
          serviceConfig = {
            ExecStart = "${pkgs.coreutils}/bin/sleep infinity";
            ExecStop = "${pkgs.bash}/bin/bash -c 'echo stopped >> /var/lib/standin/log; ${pkgs.coreutils}/bin/sync'";
          };
        };
        systemd.services.standin = {
          wantedBy = [ "multi-user.target" ];
          unitConfig.ConditionPathExists = "/var/lib/standin/mode";
          serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${standin}";
        };
      };
    };
}
