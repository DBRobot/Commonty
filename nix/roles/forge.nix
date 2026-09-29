{ config, ... }:
{
  # The forge and the mirror that keeps GitHub's copy current. Its runner
  # is the runner role: any box can run one, the forge runs on one.
  imports = [
    ./_sops.nix
    ../modules/forge/forgejo.nix
    ../modules/forge/forgejo-mirror.nix
  ];
  dd.home.services = [
    {
      name = "Code";
      blurb = "Git repositories, with their history and reviews.";
      url = "https://git.${config.dd.domain}/";
      # the forge is told nobody is there (modules/forgejo.nix): public
      # repos, no account
      demo = "full";
      icon = "code";
      color = "#4a5a8a";
      rank = 50;
    }
  ];
  dd.forgejo.admin = "david";
  dd.backup.paths = [ "/vault/forgejo" ]; # repositories, lfs, custom config; its db is in the postgres dumps
  dd.forgejo.mirrors = [
    {
      repo = config.dd.repo;
      to = "https://github.com/DBRobot/Home-Server.git";
      user = "DBRobot";
    }
  ];
}
