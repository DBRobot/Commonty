{ config, pkgs, ... }:
{
  # Looking at the fleet: our Metrics page over every box's own prometheus, alerts,
  # the mail they go out by, and the database dumps.
  imports = [
    ./_sops.nix
    ../modules/observe/metrics-ui.nix
    ../modules/observe/alerting.nix
    ../modules/mail.nix
    ../modules/storage/postgres-backup.nix
  ];
  dd.home.services = [
    {
      name = "Metrics";
      blurb = "How the boxes are doing.";
      url = "https://metrics.${config.dd.domain}/";
      # in the bar's menu beside Boxes and Backups, where looking at the
      # fleet belongs; it is not a service the way photos and films are
      menuOnly = true;
      # read-only figures: the demo sees the fleet as a member does
      demo = "full";
      icon = "metrics";
      color = "#c4562d";
      rank = 60;
    }
  ];

  dd.backup.paths = [
    "/vault/backups" # the postgres dumps: the ente key hierarchy lives there
  ];
  environment.systemPackages = with pkgs; [
    vim
    git
    htop
  ];

  # Where machine mail actually goes. Read only through the msmtp aliases
  # template below, so root-only is right.
  sops.secrets.alert-recipient = { };
  # msmtp expands local names through this, so zed, smartd and the backup
  # alerts can all address "alerts" and the real destination stays here.
  # One place to change it, and nothing names a person in the repo.
  sops.templates."msmtp-aliases".content = ''
    alerts: ${config.sops.placeholder.alert-recipient}
    default: ${config.sops.placeholder.alert-recipient}
  '';
}
