# A box a member set up, given a service by the control plane, serves it:
# the real joined box (lib.joinedBoxModules), built from its own report and
# box.json (joined-serves-vm: a house box, its household in box.json), so
# by its capability it is built with the house's Tang and carries it. It
# joins the fleet's control plane (the `fleet` machine,
# nix/tests/joined-box/fleet.py) on its own key. Under the release's rules
# for joined boxes (placement-inputs joined) it is given nothing until they
# say joined boxes are attested; with that on, the plan gives it Tang, its
# flag goes up, and Tang answers the house. Pi-hole, built in with it but
# not for a member's box, never runs.
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
  # the release's placement inputs, and the same with joined boxes attested
  release = pkg.placement-inputs;
  attested = pkgs.runCommand "placement-attested.json" { nativeBuildInputs = [ pkgs.jq ]; } ''
    jq '.joined.attested = true' ${release} > $out
  '';
in
{
  name = "joined-serves";
  node.specialArgs = { inherit self; };
  nodes = {
    box =
      { lib, ... }:
      {
        imports = self.lib.joinedBoxModules {
          name = keyName;
          dir = ./joined-serves-vm;
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
        # the plan's inputs: the test puts the release's there, then the
        # same with joined boxes attested
        dd.control.placement = "/run/dd-test/placement.json";
        # no release to move to here: the agent stays put
        systemd.services.dd-agent.enable = lib.mkForce false;
        systemd.timers.dd-agent.enable = lib.mkForce false;
        environment.systemPackages = [
          pkgs.tpm2-tools
          pkgs.curl
          pkgs.jq
        ];
      };
    fleet = {
      virtualisation.vlans = [ 1 ];
      virtualisation.memorySize = 3072;
      networking.firewall.enable = false;
      services.nginx = {
        enable = true;
        virtualHosts.fleet.root = "/srv";
      };
      environment.systemPackages = [
        pkgs.jq
        pkgs.curl
        pkgs.wrangler
        pkgs.biscuit-cli
        (pkgs.python3.withPackages (ps: [ ps.cryptography ]))
      ];
    };
  };
  scriptEnv = {
    inherit keyName;
    control = "${pkg.dd-control}/bin/dd-control";
    verify = "${pkg.verify}/bin/verify";
    worker = "${pkg.control-worker}";
    pages = "${pkg.pages}";
    fleet = ./joined-box/fleet.py;
    # the key joined-box joins with (its name is the same box's)
    key = ./joined-box-vm/test-sig.pem;
    release = "${release}";
    report = ./joined-serves-vm/hardware.json;
    attested = "${attested}";
  };
}
