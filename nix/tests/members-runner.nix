# A member's job runs in a box that is not this one. The module puts every
# container under gVisor on a network that reaches nothing of ours; here a
# container tries the tailnet, the lan and the box itself, and gets nowhere,
# while the kernel it sees is gVisor's, not the host's.
{ pkgs, self, ... }:
let
  probe = pkgs.dockerTools.buildImage {
    name = "probe";
    tag = "1";
    copyToRoot = pkgs.buildEnv {
      name = "probe-root";
      paths = [
        pkgs.busybox
        pkgs.curl
      ];
      pathsToLink = [ "/bin" ];
    };
    config.Cmd = [ "/bin/sh" ];
  };
in
{
  name = "members-runner";
  node.specialArgs = { inherit self; };
  nodes = {
    box = {
      # the gate plays no part here: without it, a change to the gate is
      # not a reason to run this again
      dd.verify.enable = false;
      imports = [
        ./box.nix
        ../modules/forge/members-runner.nix
      ];
      # no token: the runner itself would need the forge; the sandbox is
      # what is tested
      networking.firewall.enable = pkgs.lib.mkForce true;
      networking.firewall.allowedTCPPorts = [ 8080 ];
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 4096;
      # something of ours to try to reach: a service on the box
      systemd.services.ours = {
        wantedBy = [ "multi-user.target" ];
        script = "${pkgs.python3}/bin/python3 -m http.server 8080 --bind 0.0.0.0";
      };
    };
    other = {
      # a second box on the lan, also ours
      networking.firewall.enable = false;
      systemd.services.ours = {
        wantedBy = [ "multi-user.target" ];
        script = "${pkgs.python3}/bin/python3 -m http.server 8080 --bind 0.0.0.0";
      };
    };
  };
  scriptEnv = {
    python = "${pkgs.python3}/bin/python3";
    inherit probe;
  };
}
