# A game server is an instance of a template, made at run time by a member
# through the manager, and neither a deploy nor a reboot loses it. Here the
# "game" is a web server, so nothing is fetched from steam: a member asks for
# one, the guest boots and serves on the port it was given, a switch leaves
# its process alone, a reboot brings it back with its world, another member
# cannot touch it, and a stop is a clean one.
{ pkgs, self, ... }:
{
  name = "games";
  node.specialArgs = { inherit self; };
  nodes.box = {
    # the gate stays: invites and the firewall ask it who is whose friend
    imports = [
      ./box.nix
      ../modules/games/games.nix
    ];
    dd.games.enable = true;
    # a stand-in egg: nothing to install, python serves the world directory
    dd.games.catalogue = pkgs.writeText "catalogue.json" (
      builtins.toJSON {
        games = [
          {
            id = "probe";
            name = "Probe";
            description = "a web server standing in for a game";
            app = 1;
            game = null;
            cover = false;
            wine = false;
            startup = "python3 -u -m http.server {{SERVER_PORT}} --directory world";
            ready = "Serving HTTP";
            stop = "^C";
            files = { };
            install = "mkdir -p /mnt/server/world";
            settings = [
              {
                var = "SERVER_NAME";
                label = "Server name";
                default = "probe";
                kind = "text";
                editable = true;
                ours = "name";
              }
            ];
            ports = [ "SERVER_PORT" ];
          }
        ];
      }
    );
    dd.games.covers = pkgs.emptyDirectory;
    # a guest inside this guest: needs a builder with kvm, like every VM test
    # here, and nesting on (the kernel default)
    dd.games.portBase = 27015;
    dd.box.tailnet = "100.64.0.9";
    # the test framework leaves the switch script out of a test box
    system.switch.enable = true;
    environment.systemPackages = [
      pkgs.curl
      pkgs.jq
    ];
    virtualisation.memorySize = 4096;
    dd.games.memoryMiB = 1536;
    virtualisation.cores = 2;
    virtualisation.diskSize = 8192;
  };
  scriptEnv =
    { nodes, ... }:
    {
      toplevel = "${nodes.box.system.build.toplevel}";
    };
}
