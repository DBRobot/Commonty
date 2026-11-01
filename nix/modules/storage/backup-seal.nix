# A box that joined backs up as any box does (storage/backup.nix), with a
# password of its own making: there are no sops secrets for it. So that its
# backup outlives it, the password is sealed to its enlisted owner's keys, as
# the agreed identity state has them, and kept beside the repository, where
# the store's copies take it. Until it has an owner it makes none and backs
# up nothing: a stranger keeps only ciphertext copies.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  box = config.networking.hostName;
  password = "/var/lib/dd/backup-password";
  dir = "/var/lib/dd-backup-seal";
  agent = self.packages.${pkgs.stdenv.hostPlatform.system}.agent;
  joined = config.dd.box.joined or false;
in
{
  config = lib.mkIf joined {
    dd.backup.passwordFile = password;
    # none until it has an owner: nothing to run, and nothing stale
    systemd.services.restic-backups-dd.unitConfig.ConditionPathExists = "${dir}/sealed.json";
    systemd.services.dd-backup-stale.unitConfig.ConditionPathExists = "${dir}/sealed.json";

    systemd.services.dd-backup-seal = {
      description = "This box's backup password, sealed to its owner's keys";
      after = [ "dd-control.service" ];
      restartIfChanged = false;
      path = [
        pkgs.coreutils
        pkgs.rclone
      ];
      serviceConfig = {
        Type = "oneshot";
        StateDirectory = "dd-backup-seal";
        StateDirectoryMode = "0700";
      };
      # dd-control or the store not up yet is next time, never a failure
      script = ''
        set -u
        ${agent}/bin/dd-backup-seal --box ${box} --password-file ${password} --sealed ${dir}/sealed.json \
          || { echo "not sealed this time"; exit 0; }
        keys=/var/lib/dd-store/keys/backups-${box}.env
        [ -s ${dir}/sealed.json ] && [ -r "$keys" ] || exit 0
        # beside the repository, so its copies carry it
        [ ${dir}/sealed.json -nt ${dir}/sent ] || exit 0
        (
          set -a; . "$keys"; set +a
          export RCLONE_CONFIG=/dev/null RCLONE_CONFIG_HERE_TYPE=s3 RCLONE_CONFIG_HERE_PROVIDER=Other \
            RCLONE_CONFIG_HERE_ENDPOINT=http://127.0.0.1:${toString config.dd.store.port} RCLONE_CONFIG_HERE_REGION=us-east-1 \
            RCLONE_CONFIG_HERE_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID \
            RCLONE_CONFIG_HERE_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY \
            RCLONE_CONFIG_HERE_FORCE_PATH_STYLE=true
          rclone copyto --s3-no-check-bucket ${dir}/sealed.json here:backups-${box}/dd-devices.json
        ) && touch ${dir}/sent || echo "not put beside the repository yet"
        exit 0
      '';
    };
    systemd.timers.dd-backup-seal = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "5min";
        OnUnitActiveSec = "1h";
      };
    };
  };
}
