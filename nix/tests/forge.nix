# The forge, booted: its setup units make the admin and protect main. The
# layout move once left the admin's name unexpanded in that setup and no
# test here booted a forge to notice; now one does.
{ pkgs, self, ... }:
{
  name = "forge";
  node.specialArgs = { inherit self; };
  nodes.box = {
    imports = [
      ./box.nix
      ../modules/forge/forgejo.nix
    ];
    dd.forgejo.admin = "tester";
    # the full gate: the Git pages' live changes come through it
    dd.verify.role = pkgs.lib.mkForce "full";
    dd.forgejo.ciSecretFile = pkgs.writeText "ci-secret" "test-ci-secret";
    # the forge's state dir is a dataset on a box; here a directory, and a
    # stand-in for the unit that would make it
    systemd.services.zfs-datasets = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = "mkdir -p /vault";
    };
    environment.systemPackages = [
      pkgs.curl
      pkgs.jq
      pkgs.postgresql
    ];
    virtualisation.memorySize = 2048;
    virtualisation.diskSize = 4096;
  };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
    admin = "tester";
    port = 3001; # nothing listens here now; the test checks that
    sock = "/run/forgejo/forgejo.sock";
  };
}
