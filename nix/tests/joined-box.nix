# A joined box's own system, end to end after its first boot (the boot
# chain before it is join-install's): the real joined box (lib.joinedBox's
# modules), installed under the name `joining`, with its key to join with
# in its TPM. Its agent names it by the key and moves it to that name's
# system, built for it ahead here, as the flake the release signs gives
# it; its dd-control then joins the fleet's control plane with its
# member's ticket, and the member's gate says the box is hers. Offline:
# the fleet is the `fleet` machine (nix/tests/joined-box/fleet.py).
{
  pkgs,
  lib,
  self,
  ...
}:
let
  sys = pkgs.stdenv.hostPlatform.system;
  pkg = self.packages.${sys};
  # the test's key to join with (joined-box-vm/test-sig.pem), and the name
  # it gives (state.JoinName)
  keyName = "j4b4145163c";
  box =
    name:
    { lib, ... }:
    {
      imports = self.lib.joinedBoxModules {
        inherit name;
        dir = ./joined-box-vm;
      };
      virtualisation.tpm.enable = true;
      virtualisation.memorySize = 3072;
      virtualisation.vlans = [ 1 ];
      # the same address under either name: the switch between them is the
      # box's own, not the test's network
      networking.interfaces.eth1.ipv4.addresses = lib.mkForce [
        {
          address = "192.168.1.50";
          prefixLength = 24;
        }
      ];
      # the test's network, not the house's
      networking.networkmanager.enable = lib.mkForce false;
      # its disk's boot is join-install's test; this one boots the test's
      dd.unlock.waits.enrolment = 1;
      dd.control.relay = "ws://fleet:8789/relay";
      environment.etc."dd-control/config.json".source = lib.mkForce "/run/dd-test/config.json";
      dd.agent.url = lib.mkForce "http://fleet/release/current.json";
      dd.agent.publicKeyFile = lib.mkForce "/run/dd-test/release.pub";
      dd.agent.probeSeconds = lib.mkForce 60;
      systemd.services.dd-agent.environment.DD_AGENT_JOINED_SOURCE =
        lib.mkForce "git+http://fleet/flake.git";
      environment.systemPackages = [
        pkgs.tpm2-tools
        pkgs.openssl
        pkgs.curl
      ];
    };
in
{
  name = "joined-box";
  node.specialArgs = { inherit self; };
  nodes = {
    box =
      { nodes, ... }:
      {
        imports = [ (box "joining") ];
        # the system its key's name gives, already in its store
        system.extraDependencies = [ nodes.boxNamed.system.build.toplevel ];
        environment.etc."dd-test/named".text = "${nodes.boxNamed.system.build.toplevel}";
      };
    # never started: the system the box moves to
    boxNamed = box keyName;
    fleet = {
      virtualisation.vlans = [ 1 ];
      virtualisation.memorySize = 3072;
      networking.firewall.enable = false;
      services.nginx = {
        enable = true;
        virtualHosts.fleet.root = "/srv";
      };
      environment.systemPackages = [
        pkgs.git
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
    dd = "${pkg.dd}/bin/dd";
    control = "${pkg.dd-control}/bin/dd-control";
    verify = "${pkg.verify}/bin/verify";
    worker = "${pkg.control-worker}";
    pages = "${pkg.pages}";
    fleet = ./joined-box/fleet.py;
    key = ./joined-box-vm/test-sig.pem;
  };
}
