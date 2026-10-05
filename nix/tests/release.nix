# A box moves itself to a signed release and to nothing else. The forge
# here is a web server with one file; the release key is made in the test
# and the box gets only its public half. A good release switches the box,
# an older counter is refused, a forged signature is refused, and a release
# that leaves the box unhealthy rolls back on its own.
{ pkgs, self, ... }:
let
  dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
in
{
  name = "release";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1536;
  defaults.virtualisation.cores = 2; # services start in parallel instead of queueing on one thread
  defaults.virtualisation.diskSize = 4096;
  nodes = {
    forge = {
      services.nginx = {
        enable = true;
        virtualHosts.forge.root = "/srv/release";
      };
      networking.firewall.enable = false;
      environment.systemPackages = [ pkgs.git ];
    };
    box = {
      imports = [
        ./box.nix
        ../modules/box/agent.nix
      ];
      services.openssh.enable = true;
      dd.agent = {
        enable = true;
        url = "http://forge/current.json";
        publicKeyFile = "/root/release.pub"; # made below, not at build time
        probeSeconds = 20;
        # the forge stands in for another box: what this one must still
        # reach after a switch
        reach = [ "forge:80" ];
      };
      # the same box with a mark on it: what a release moves it to
      specialisation.marked.configuration.environment.etc."dd-marker".text = "release 2\n";
      # and a broken one: ssh gone, which the probe must catch
      specialisation.broken.configuration = {
        environment.etc."dd-marker".text = "release 3\n";
        services.openssh.enable = pkgs.lib.mkForce false;
      };
      # healthy in every way this box can see, and cut off from the fleet:
      # the shape of a network change that strands a box
      specialisation.stranded.configuration = {
        environment.etc."dd-marker".text = "release 4\n";
        networking.firewall.enable = pkgs.lib.mkForce true;
        # cut off: nothing comes back, as when the network is gone. (A
        # refusal would be an answer - the agent counts one as reach.)
        networking.firewall.extraCommands = ''
          iptables -I OUTPUT -p tcp --dport 80 -j DROP
          ip6tables -I OUTPUT -p tcp --dport 80 -j DROP
        '';
      };
    };
  };
  scriptEnv =
    { nodes, ... }:
    {
      inherit dd;
      base = "${nodes.box.system.build.toplevel}";
      marked = "${nodes.box.specialisation.marked.configuration.system.build.toplevel}";
      broken = "${nodes.box.specialisation.broken.configuration.system.build.toplevel}";
      stranded = "${nodes.box.specialisation.stranded.configuration.system.build.toplevel}";
    };
}
