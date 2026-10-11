{ config, ... }:
{
  # Game servers a member starts for themselves (nix/modules/games/games.nix): every
  # steam dedicated server the pelican eggs know, installed anonymously by
  # steamcmd in a guest of its own. That a given server exists is the box's
  # state, not the repo's.
  imports = [ ../modules/games/games.nix ];

  dd.games.enable = true;

  dd.home.services = [
    {
      name = "Games";
      blurb = "Start a game server and play with the people you invite.";
      url = "https://games.${config.dd.domain}/";
      icon = "games";
      color = "#7a4fb5";
      rank = 35;
      # the demo may look at what runs here, not start anything
      demo = "read";
    }
  ];

  # each server's directory (the game and its world, as the egg lays them
  # out), not the guest's own disk; its record and who its owner invited,
  # which the host goes by and the guest cannot write: without them a
  # restored directory is no one's server (Manager::new will not make one
  # from the guest's copy); and the worlds a member kept on purpose, which
  # outlive the server they came from
  dd.backup.paths = [
    "/var/lib/dd-games/instances"
    "/var/lib/dd-games/records"
    "/var/lib/dd-games/players"
    "/var/lib/dd-games/worlds"
  ];
  dd.backup.exclude = [ "/var/lib/dd-games/instances/*/disk.qcow2" ];
}
