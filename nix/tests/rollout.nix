# Staged rollout (docs/dev/rollout.md) on two boxes and a relay: the real
# dd-control, agreeing with the witness under wrangler, and the real
# dd-agent following its record. One writer, "web", is placed by the plan
# under a lease, behind each box's nginx with the hold page. Releases move
# the boxes to their specialisations:
#   a good release goes to wave 1, soaks, and then to the other box, which
#   hands web over before it switches;
#   a release that fails its soak on wave 1 stops there and rolls back;
#   a box whose switch fails has handed web over first, and a member's
#   probe of web never fails while it fails and rolls back;
#   the control plane down mid-rollout holds everyone where they are.
{ pkgs, self, ... }:
let
  sys = pkgs.stdenv.hostPlatform.system;
  pkg = self.packages.${sys};
  dd = "${pkg.dd}/bin/dd";
  placement = pkgs.writeText "placement.json" (
    builtins.toJSON {
      boxes = pkgs.lib.genAttrs [ "node1" "node2" ] (_: {
        site = "h1";
        region = "r";
        can.web = 1;
        memoryGiB = 2;
        threads = 2;
        poolGiB = 4;
      });
      services.web = {
        enabled = true;
        instances = 1;
        class = "low";
        placed = true;
        lease = true;
        memoryGiB = 0.05;
      };
      roles = { };
      damping = {
        base = 0.3;
        perGiB = 0.01;
      };
    }
  );
  box =
    { config, lib, ... }:
    {
      imports = [
        ./box.nix
        ../modules/box/agent.nix
      ];
      services.openssh.enable = true;
      virtualisation.memorySize = 1536;
      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
      ];
      dd.agent = {
        enable = true;
        url = "http://relay/current.json";
        publicKeyFile = "/root/release.pub";
        probeSeconds = 20;
        reach = [ "relay:80" ];
        rollout = true;
        drainSeconds = 120;
        interval = "20s";
      };
      # what this box's soak probes: web, while it holds it
      environment.etc."rollout/site-probes.json".text = builtins.toJSON [
        {
          flag = "web";
          url = "http://web.test.invalid/";
          resolve = "web.test.invalid:80:127.0.0.1";
        }
      ];
      systemd.services.dd-agent.environment.DD_AGENT_SITE_PROBES =
        lib.mkForce "/etc/rollout/site-probes.json";
      environment.etc."dd-placed/carries.json".text = builtins.toJSON { web = [ "web.service" ]; };
      systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
      # dd-control as box/control.nix runs it, with this fleet's config and
      # a test key in place of the TPM; started once the test has made them
      systemd.services.dd-control = {
        restartIfChanged = false;
        path = [ pkgs.systemd ];
        serviceConfig = {
          ExecStart = lib.concatStringsSep " " [
            "${pkg.dd-control}/bin/dd-control"
            "-config /var/lib/rollout/config.json"
            "-box ${config.networking.hostName}"
            "-relay ws://relay:8789/relay"
            "-state /var/lib/dd-control"
            "-box-key /var/lib/rollout/box.p8"
            "-listen 127.0.0.1:4195"
            "-heartbeat 2s"
            "-placement ${placement}"
            "-placed-carries /etc/dd-placed/carries.json"
            "-rollouts /run/dd-control/rollout.sock"
          ];
          StateDirectory = "dd-control";
          RuntimeDirectory = "dd-control";
          Restart = "always";
          RestartSec = 2;
        };
      };
      # the writer: a page saying where it runs, only where its flag is up
      environment.etc."web/index.html".text = "web on ${config.networking.hostName}\n";
      systemd.services.web = {
        unitConfig.ConditionPathExists = "/run/dd-placed/web";
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 -m http.server 8080 --bind 127.0.0.1 --directory /etc/web";
      };
      # its name, with the hold page while it is not here (gate/verify.nix)
      services.nginx.virtualHosts."web.test.invalid" = {
        extraConfig = "error_page 502 504 =503 /_dd/moving/moving.html;";
        locations."/".proxyPass = "http://127.0.0.1:8080";
        locations."/_dd/moving/" = {
          alias = "${../../servers/web-shared/moving}/";
          extraConfig = ''
            add_header Retry-After 5 always;
          '';
        };
        # what a member's name follows: the agreed lease (the DNS Worker)
        locations."= /_test/state".proxyPass = "http://127.0.0.1:4195/state";
      };
      # releases: a marker each, one with a crash loop that starts after
      # the switch has probed, one without ssh
      specialisation = {
        good.configuration.environment.etc."dd-marker".text = "good\n";
        good2.configuration.environment.etc."dd-marker".text = "good2\n";
        good3.configuration.environment.etc."dd-marker".text = "good3\n";
        crashloop.configuration = {
          environment.etc."dd-marker".text = "crashloop\n";
          systemd.services.flaky = {
            wantedBy = [ "multi-user.target" ];
            startLimitIntervalSec = 0;
            serviceConfig = {
              Restart = "always";
              RestartSec = 1;
            };
            script = ''
              if [ -e /run/flaky-once ]; then sleep 2; else touch /run/flaky-once; sleep 25; fi
              exit 1
            '';
          };
        };
        broken.configuration = {
          environment.etc."dd-marker".text = "broken\n";
          services.openssh.enable = lib.mkForce false;
        };
      };
    };
in
{
  name = "rollout";
  helpers = true;
  node.specialArgs = { inherit self; };
  defaults.virtualisation.cores = 2;
  defaults.virtualisation.diskSize = 4096;
  nodes = {
    relay = {
      virtualisation.memorySize = 1536;
      networking.firewall.enable = false;
      services.nginx = {
        enable = true;
        virtualHosts.relay.root = "/srv/release";
      };
      environment.systemPackages = [
        pkgs.git
        pkgs.openssl
        pkgs.wrangler
        pkgs.python3
        pkgs.curl
      ];
    };
    node1 = box;
    node2 = box;
  };
  scriptEnv =
    { nodes, ... }:
    {
      inherit dd;
      worker = "${pkg.control-worker}";
      control = "${pkg.dd-control}/bin/dd-control";
      paths = builtins.mapAttrs (
        _: n:
        {
          base = "${n.system.build.toplevel}";
        }
        // builtins.mapAttrs (_: s: "${s.configuration.system.build.toplevel}") n.specialisation
      ) { inherit (nodes) node1 node2; };
    };
}
