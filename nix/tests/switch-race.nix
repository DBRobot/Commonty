# A switch while the owner's ssh sessions come and go. node1 refused release
# 314 because the laptop's queue logs in as admin every few seconds: each
# login started admin's systemd and each logout stopped it, and a switch
# that reached admin's systemd while it was going failed with status 4.
# Here a loop logs in and out as admin over ssh the whole time, and the box
# switches again and again; every switch must succeed, and admin's systemd
# must be the same one throughout.
{ pkgs, ... }:
let
  keys = import "${pkgs.path}/nixos/tests/ssh-keys.nix" pkgs;
in
{
  name = "switch-race";
  helpers = true;
  nodes.box =
    { lib, ... }:
    {
      imports = [ ../modules/box/logins.nix ];
      system.switch.enable = true;
      users.users.admin = {
        isNormalUser = true;
        extraGroups = [ "wheel" ];
        openssh.authorizedKeys.keys = [ keys.snakeOilPublicKey ];
      };
      services.openssh.enable = true;
      # logind's 10s before stopping a user's systemd only spaces the same
      # race out; at 0 every logout stops it, so a switch meets it often
      services.logind.settings.Login.UserStopDelaySec = 0;
      # the queue: log in as admin, run nothing much, log out, again
      systemd.services.churn = {
        path = [
          pkgs.openssh
          pkgs.coreutils
        ];
        script = ''
          install -m 600 ${keys.snakeOilPrivateKey} /run/churn-key
          n=0
          while true; do
            ssh -i /run/churn-key -o BatchMode=yes -o StrictHostKeyChecking=no \
              -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR admin@127.0.0.1 true || true
            n=$((n + 1))
            echo "$n" > /run/churn-logins
            sleep 0.$((RANDOM % 4))
          done
        '';
      };
    };
}
