{
  ddScript,
  config,
  pkgs,
  ...
}:
let
  root = "/srv/users";
  images = "/srv/images";
  directory = "/var/lib/dd-verify/keys";

  # Every per-user directory, derived from the directory at runtime rather
  # than declared here. Who exists is whoever published a signed entry; there
  # is no list to keep and no server to ask.
  #
  # The uid is a hash of the name. Every box computes the same one from the
  # same entry, so shared storage carries a number that means the same
  # everywhere and nobody maintains a map. No nss entry exists for it: the
  # files are owned by a number, and every reader here (nginx) gets
  # in through an acl, never through the name.
  sync = pkgs.writeShellScript "user-accounts-sync" (
    ddScript ./user-accounts.sh {
      DIRECTORY = directory;
      IMAGES = images;
      ROOT = root;
    }
  );
in
{
  systemd.services.user-accounts = {
    description = "Per-user directories, from the directory";
    after = [
      "zfs-mount.service"
      "zfs-datasets.service"
      "dd-verify.service"
    ];
    requires = [ "zfs-datasets.service" ];
    wantedBy = [ "multi-user.target" ];
    path = [
      pkgs.acl
      pkgs.coreutils
      pkgs.gnugrep
    ];
    # Every five minutes, not just at activation: an identity published in
    # between would have no directory until the next rebuild, and webdav
    # would answer 500 for them. RemainAfterExit is deliberately NOT set: a
    # oneshot that stays "active (exited)" cannot be re-triggered by a timer.
    startAt = "*:0/5";
    serviceConfig = {
      Type = "oneshot";
      ExecStart = sync;
    };
  };
}
