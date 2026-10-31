# A member's request reaches a service the agreed plan placed on a box the
# release does not list. The real joined box (lib.joinedBoxModules), built to
# run the games, given dd-games by the real plan under the release's rules
# with joined boxes attested and no listed box to prefer (attestation itself
# is its own test's). The `fleet` machine is the control plane
# (joined-box/fleet.py), its gate and a real headscale with the real policy
# and plan unit; ada and bob are members' devices, `stranger` a box that
# joined and that nobody enlisted.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  sys = pkgs.stdenv.hostPlatform.system;
  pkg = self.packages.${sys};
  keyName = "j4b4145163c";
  attested = pkgs.runCommand "placement-attested.json" { nativeBuildInputs = [ pkgs.jq ]; } ''
    jq '.joined.attested = true | .boxes = {}' ${pkg.placement-inputs} > $out
  '';
  device = {
    services.tailscale.enable = true;
    virtualisation.memorySize = 1024;
    virtualisation.vlans = [ 1 ];
    networking.useDHCP = false;
    environment.systemPackages = [
      pkgs.netcat
      pkgs.jq
      pkgs.curl
    ];
  };
in
{
  name = "joined-reach";
  helpers = true;
  node.specialArgs = { inherit self; };
  nodes = {
    box =
      { lib, ... }:
      {
        imports = self.lib.joinedBoxModules {
          name = keyName;
          dir = ./joined-reach-vm;
        };
        virtualisation.tpm.enable = true;
        virtualisation.memorySize = 3072;
        virtualisation.emptyDiskImages = [ 2048 ];
        virtualisation.vlans = [ 1 ];
        networking.interfaces.eth1.ipv4.addresses = lib.mkForce [
          {
            address = "192.168.1.50";
            prefixLength = 24;
          }
        ];
        networking.networkmanager.enable = lib.mkForce false;
        dd.unlock.waits.enrolment = 1;
        dd.control.relay = "ws://fleet:8789/relay";
        environment.etc."dd-control/config.json".source = lib.mkForce "/run/dd-test/config.json";
        dd.control.placement = "${attested}";
        dd.games.door = lib.mkForce "http://fleet:4281/_dd/network/games";
        # it joins by the test's key, not through the front door
        systemd.services.commonty-net-up.enable = lib.mkForce false;
        systemd.services.commonty-net-bridge.enable = lib.mkForce false;
        systemd.services.dd-agent.enable = lib.mkForce false;
        systemd.timers.dd-agent.enable = lib.mkForce false;
        environment.systemPackages = [
          pkgs.tpm2-tools
          pkgs.curl
          pkgs.jq
          pkgs.netcat
          pkgs.tailscale
          pkgs.iptables
        ];
      };
    fleet =
      { lib, ... }:
      {
        imports = [
          ./box.nix
          ../modules/net/headscale.nix
          ../modules/net/box.nix
          ../modules/net/owner.nix
        ];
        virtualisation.vlans = [ 1 ];
        virtualisation.memorySize = 3584;
        virtualisation.cores = 2;
        dd.domain = lib.mkForce "commonty.org";
        dd.verify.role = lib.mkForce "full";
        # the gates are the fleet's own, below; this one would reap the devices
        systemd.services.dd-verify.enable = lib.mkForce false;
        services.headscale.address = lib.mkForce "0.0.0.0";
        dd.headscale = {
          enable = true;
          url = "http://fleet:8085";
          behindNginx = false;
          admin = "sarah";
          control = "http://127.0.0.1:4191";
          gamePorts = (import ../lib/games-reach.nix { inherit lib; }).reach;
        };
        services.headscale.settings.derp = {
          urls = lib.mkForce [ ];
          paths = [
            "${pkgs.writeText "derp-test.yaml" ''
              regions:
                900:
                  regionid: 900
                  regioncode: test
                  regionname: Test
                  nodes:
                    - name: 900a
                      regionid: 900
                      hostname: fleet
                      ipv4: 127.0.0.1
                      stunport: 3478
                      derpport: 3479
            ''}"
          ];
        };
        # a name of the games on the box running the network, as on node1;
        # its nginx off, with no certificate here: the test serves the fleet's
        # files itself
        services.nginx.virtualHosts."games.commonty.org".root = "/srv";
        environment.systemPackages = [
          pkgs.jq
          pkgs.curl
          pkgs.wrangler
          pkgs.biscuit-cli
          pkgs.netcat
          pkgs.tailscale
          (pkgs.python3.withPackages (ps: [ ps.cryptography ]))
        ];
      };
    ada = device;
    bob = device;
    stranger = device;
  };
  scriptEnv = {
    inherit keyName;
    control = "${pkg.dd-control}/bin/dd-control";
    verify = "${pkg.verify}/bin/verify";
    worker = "${pkg.control-worker}";
    pages = "${pkg.pages}";
    fleet = ./joined-box/fleet.py;
    key = ./joined-box-vm/test-sig.pem;
    attested = "${attested}";
  };
}
