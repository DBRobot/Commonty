# Whoever can log in to a box keeps their own systemd running. A switch
# reloads every logged-in user's systemd, and fails (status 4) when one of
# them is half-gone: an ssh session as admin ended, its systemd was stopping,
# and the switch's per-user step found /run/user/1000 already unmounted
# ("Failed to create /run/user/1000/nixos: Permission denied") or the
# manager mid-exit ("default.target/start is destructive"). The laptop's
# queue logs in as admin every few seconds, and node1 refused release 314
# twice over for it. Lingering, the manager starts at boot and a session
# coming or going no longer starts or stops it. forgejo, whose ssh pushes
# did the same to 244 and 246, lingers from its own module.
{ config, lib, ... }:
let
  # a person, or anyone ssh lets in by key
  logsIn = lib.filterAttrs (
    n: u:
    n != "root"
    && (
      u.isNormalUser || u.openssh.authorizedKeys.keys != [ ] || u.openssh.authorizedKeys.keyFiles != [ ]
    )
  ) config.users.users;
in
{
  users.users.admin.linger = true;
  assertions = lib.mapAttrsToList (n: u: {
    assertion = u.linger == true;
    message = "users.users.${n} can log in, so it must linger (nix/modules/box/logins.nix): a session ending mid-switch fails the release";
  }) logsIn;
}
