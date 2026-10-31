# A member's remote computer on a box, the box's side: what dd-control does
# (its flag, held under the computer's lease, and systemctl) done by the
# test, and what it serves (/computers, /plan) by a stand-in on its port. The
# control plane's side - judged, placed, leased, started and stopped - is
# the control test's (control/run.py), with real validators.
#
# Then the whole road, as a member takes it from their laptop: `dd
# computer config`, `make`, `ssh`, `stop` and `start`, through nginx's
# compute.<domain> and home.<domain> and the gate in front of them, to the
# box's own dd-control, which judges, places and leases it with the relay's
# witness (computers/fleet.py) and starts it here.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  sys = pkgs.stdenv.hostPlatform.system;
  pkg = self.packages.${sys};
  domain = "test.invalid";
  cert = import ../lib/test-cert.nix { inherit pkgs; } {
    subject = domain;
    names = [ "*.${domain}" ];
  };
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
  # The Commonty network as a computer sees it: a control server somewhere
  # on the web (the test's web namespace, below), by address, with its own
  # relay, and the rules a computer and a box get from the real one
  # (net/headscale.nix): a computer reaches a box's 443, and no more
  control = "198.51.100.2";
  controlCert = import ../lib/test-cert.nix { inherit pkgs; } {
    subject = control;
    names = [ ];
    ips = [ control ];
    name = "control-cert";
  };
  netPolicy = pkgs.writeText "policy.json" (
    builtins.toJSON {
      tagOwners."tag:box" = [ "boxes@" ];
      tagOwners."tag:computer" = [ "boxes@" ];
      acls = [
        {
          action = "accept";
          src = [ "tag:computer" ];
          dst = [ "tag:box:443" ];
        }
        {
          action = "accept";
          src = [ "tag:box" ];
          dst = [ "tag:box:*" ];
        }
      ];
    }
  );
  # An owner's Nix file applied live (`dd computer apply`). The guest builds
  # its system itself, and has no cache in here: what that build takes
  # beyond the image (a stdenv, and the like; the bridge is a store path to
  # the guest and a derivation here, so those few derivations differ from
  # the image's) is handed in first, as its owner could (nix copy, as a
  # trusted user of its nix)
  applied = ./computers/applied.nix;
  appliedGuest =
    (import ../modules/computers/guest/system.nix {
      inherit pkgs;
      inherit (pkg) netbridge;
      member = applied;
    }).config;
  appliedInputs = map (d: d.inputDerivation) [
    appliedGuest.system.build.toplevel
    appliedGuest.system.build.etc
    appliedGuest.environment.etc."systemd/system".source
  ];
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
  # about 5 minutes on a quiet node1, 30 on a busy runner; one that hangs fails long
  # before CI's hour
  globalTimeout = 2700;
  node.specialArgs = { inherit self; };
  nodes.box =
    { config, ... }:
    {
      imports = [
        ./box.nix
        ../modules/computers/computers.nix
        ../modules/box/control.nix
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

      # the whole road: the box's own dd-control (box/control.nix), with the
      # test fleet's config and a key in place of its TPM's, started once
      # computers/fleet.py has made them
      dd.control.enable = true;
      environment.etc."dd-control/config.json".source = lib.mkForce "/run/dd-test/config.json";
      systemd.services.dd-control = {
        wantedBy = lib.mkForce [ ];
        unitConfig.ConditionPathExists = "/run/dd-test/config.json";
        serviceConfig.ExecStart = lib.mkForce (
          lib.concatStringsSep " " [
            "${pkg.dd-control}/bin/dd-control"
            "-config /etc/dd-control/config.json"
            "-box box"
            "-relay ws://127.0.0.1:8789/relay"
            "-state /var/lib/dd-control"
            "-box-key /run/dd-test/box.p8"
            "-listen 127.0.0.1:4195"
            "-heartbeat 2s"
            "-placement /run/dd-test/placement.json"
            "-placed-carries /etc/dd-placed/carries.json"
            "-placed-starts /etc/dd-placed/starts.json"
            "-submit /run/dd-control/submit.sock -submit-group dd-verify"
            "-builds /run/dd-control/built.sock"
            "-rollouts /run/dd-control/rollout.sock"
            "-metrics /var/lib/dd-facts/control.prom"
          ]
        );
      };
      # the gate in front of compute.<domain> and home.<domain>, as on a box
      # that serves its members, reading who is who from the agreed state
      dd.verify.role = lib.mkForce "full";
      services.nginx.enable = true;
      services.nginx.virtualHosts =
        lib.genAttrs (map (h: "${h}.${domain}") config.dd.verify.hosts) (_: {
          useACMEHost = lib.mkForce null;
          sslCertificate = "${cert}/cert.pem";
          sslCertificateKey = "${cert}/key.pem";
        })
        // {
          # dd trusts only the web's certificates, so the test reaches the
          # box's https names through a plain door of its own; Host is kept
          "test-door" = {
            serverName = "home.${domain}";
            serverAliases = [ "compute.${domain}" ];
            listen = [
              {
                addr = "127.0.0.1";
                port = 8080;
              }
            ];
            locations."/" = {
              proxyPass = "https://127.0.0.1";
              extraConfig = ''
                proxy_set_header Host $host;
                proxy_ssl_server_name on;
                proxy_ssl_name $host;
              '';
            };
          };
        };
      networking.hosts."127.0.0.1" = [
        "home.${domain}"
        "compute.${domain}"
      ];
      # where the box is on the Commonty network, as fleet/boxes.json says of
      # a real one: here, its loopback
      systemd.services.dd-computers.environment.DD_COMPUTERS_BOXES = lib.mkForce "${pkgs.writeText
        "boxes.json"
        (builtins.toJSON { box = "127.0.0.1"; })
      }";
      # somewhere on the web, as a guest sees it: a namespace of the test's
      # own (web), at a documentation address, neither the box nor private
      systemd.services.web-far.serviceConfig = {
        NetworkNamespacePath = "/run/netns/web";
        ExecStart = "${pkgs.python3}/bin/python3 -m http.server 80 --bind 198.51.100.2 --directory /srv/web";
      };
      networking.dhcpcd.denyInterfaces = [ "web0" ];
      # the control server, out there with the web: started once the test
      # has made the namespace
      services.headscale = {
        enable = true;
        address = control;
        port = 443;
        settings = {
          server_url = "https://${control}";
          tls_cert_path = "${controlCert}/cert.pem";
          tls_key_path = "${controlCert}/key.pem";
          derp = {
            urls = [ ];
            server = {
              enabled = true;
              region_id = 999;
              region_code = "test";
              region_name = "Test";
              stun_listen_addr = "${control}:3478";
              automatically_add_embedded_derp_region = true;
            };
          };
          dns = {
            magic_dns = false;
            override_local_dns = false;
            base_domain = "net.${domain}";
          };
          policy = {
            mode = "file";
            path = "${netPolicy}";
          };
        };
      };
      systemd.services.headscale = {
        wantedBy = lib.mkForce [ ];
        serviceConfig.NetworkNamespacePath = "/run/netns/web";
      };
      # the box on the network too, as a box joins it (tag:box)
      services.tailscale.enable = true;
      security.pki.certificateFiles = [ "${controlCert}/cert.pem" ];
      networking.firewall.interfaces.tailscale0.allowedTCPPorts = [
        443
        4998
      ];
      system.extraDependencies = appliedInputs;
      # the fence as box/placed.nix has it
      environment.etc."dd-placed/carries.json".text = "{}";
      environment.etc."dd-placed/starts.json".text = "{}";
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
        (pkgs.python3.withPackages (ps: [ ps.cryptography ]))
        pkgs.wrangler
        pkgs.iproute2
      ];
      # a 1G guest inside this one, beside the relay: needs a builder with
      # kvm, nesting on
      virtualisation.memorySize = 4096;
      virtualisation.cores = 2;
      virtualisation.diskSize = 16384;
    };
  scriptEnv = {
    dd = "${pkg.dd}/bin/dd";
    control = "${pkg.dd-control}/bin/dd-control";
    worker = "${pkg.control-worker}";
    fleet = ./computers/fleet.py;
    inherit applied;
    appliedInputs = lib.concatStringsSep " " appliedInputs;
    ca = "${controlCert}/cert.pem";
  };
}
