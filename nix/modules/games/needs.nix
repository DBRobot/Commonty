# What game servers need of a box (types: nix/lib/needs-type.nix). Every
# game in data/games/catalogue.json is a variant: what it takes is worked
# out from the catalogue, so a new game needs nothing written here.
let
  catalogue = builtins.fromJSON (builtins.readFile ../../../data/games/catalogue.json);
  variant = g: {
    name = g.id;
    value = {
      requires = {
        # the manager's sizes (box/games/src/lib.rs, Game::memory/cores):
        # wine servers want more; nothing in an egg says
        memory.fixed = if g.wine or false then "8G" else "6G";
        threads = 4;
        # installs are several GB; not measured per game yet
        disk.size.min = "15G";
      };
    };
  };
in
{
  dd-games = {
    what = "game servers members start for themselves, each in its own VM";
    base = {
      requires = {
        # not overfit: every server in the catalogue is an x86 binary
        # (steamcmd, wine)
        arch = [ "x86_64" ];
        kvm = true;
        disk.size.min = "100G"; # a few worlds' installs and their saves
      };
      # node1, 2026-10-04: the manager; worlds are what the variants take
      uses = {
        memory = "5M";
        storage = "1M";
      };
      network = {
        outbound = "internet"; # steam's CDN, GitHub, from inside the guests
        inbound = "tailnet"; # players, over the Commonty network only
      };
      depends = {
        nginx.local = true;
        dd-verify.local = true;
        commonty-net.local = true; # the firewall opens worlds on commonty0
      };
      privacy = "plaintext"; # members' worlds and saves
    };
    variants = builtins.listToAttrs (map variant catalogue.games);
  };
}
