{ config, pkgs, ... }:
{
  # Looking at the fleet: our Metrics page over every box's own prometheus, alerts,
  # and the mail they go out by. Database dumps are each postgres box's own
  # (storage/postgres-backup.nix, through core).
  imports = [
    ./_sops.nix
    ../modules/observe/metrics-ui.nix
    ../modules/observe/alerting.nix
    ../modules/mail.nix
  ];
  dd.home.services = [
    {
      name = "Metrics";
      blurb = "How the boxes are doing.";
      url = "https://metrics.${config.dd.domain}/";
      # in the bar's menu beside Boxes and Backups, where looking at the
      # fleet belongs; it is not a service the way photos and films are
      menuOnly = true;
      # no demo: the fleet's figures are members' business, and a demo door
      # would put them on the open internet
      icon = "metrics";
      color = "#c4562d";
      rank = 60;
    }
  ];

  environment.systemPackages = with pkgs; [
    vim
    git
    htop
  ];
}
