# What game servers need of a box (types: nix/lib/needs-type.nix). Every
# game in data/games/catalogue.json is a variant: what it takes is worked
# out from the catalogue, so a new game needs nothing written here.
let
  catalogue = builtins.fromJSON (builtins.readFile ../../../data/games/catalogue.json);
  variant = g: {
    name = g.id;
    value = {
      requires = {
        # what the manager gives its guest: the catalogue's figure for it
        # (data/games/resolve.py, MEMORY), and Game::cores in lib.rs
        memory.fixed = "${toString g.memory_mib}M";
        threads = 4;
        # its install as Steam lists it (data/games/sizes.py), half again for
        # updates and saves, and at least 2G; 15G where Steam does not say
        disk.size.min =
          if g ? disk_bytes then
            let
              gib = g.disk_bytes * 3 / 2 / (1024 * 1024 * 1024) + 1;
            in
            "${toString (if gib < 2 then 2 else gib)}G"
          else
            "15G";
      };
    };
  };
in
{
  dd-games = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "game servers members start for themselves, each in its own VM";
    # members' worlds and saves: on their own box, or the fleet owner's
    trust = "own";
    # games.<domain>: the DNS Worker points it at the box running this
    hosts = [ "games" ];
    instances = 1;
    # members' worlds are on this box's disk, not in a bucket: they stay
    # while it is up, and the games move only when it is gone
    moves = "on-failure";
    # players connect to it; a world runs when someone starts it
    traffic = {
      class = "high";
      upMbps = 10;
    };
    units = [
      "dd-game@*"
      "dd-games-measure"
      "games-access"
      "dd-games-usage"
    ];
    base = {
      requires = {
        # not overfit: every server in the catalogue is an x86 binary
        # (steamcmd, wine)
        arch = [ "x86_64" ];
        kvm = true;
        disk.size.min = "100G"; # a few worlds' installs and their saves
        # the worlds are on this dataset and nowhere else (backed up, not
        # copied): only a box whose owner gave it one runs the games
        dataset = "/var/lib/dd-games";
      };
      # node1, 2026-10-04: the manager; worlds are what the variants take
      uses = {
        memory = "5M";
        storage = "1M";
      };
      network = {
        outbound = "internet"; # steam's CDN, GitHub, from inside the guests
        inbound = "network"; # players, over the Commonty network only
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
