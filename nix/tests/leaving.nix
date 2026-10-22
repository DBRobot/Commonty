# Leaving (docs/dev/leaving.md): a member leaves with their root's signed
# word, every way in ends at once and the gate deletes their libraries, disk
# images, notes and names straight away; a member the owner removes keeps
# everything for the window, then loses it the same way; another
# member's things are never touched.
{ pkgs, self, ... }:
{
  name = "leaving";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  defaults.virtualisation.diskSize = 4096;
  nodes.box =
    { lib, ... }:
    {
      imports = [
        ./box.nix
        ../modules/storage/store.nix
        ../modules/library/libraries.nix
      ];
      dd.store = {
        enable = true;
        capacity = "1G";
        agePlugins = [ ];
        # this box writes them: the gate's keys are the store's
        buckets.libraries.writer = "box";
        buckets.mail.writer = "box";
      };
      # no certificate in a test, and no other box reaches this store
      services.nginx.virtualHosts."store.box.test.invalid" = {
        useACMEHost = lib.mkForce null;
        onlySSL = lib.mkForce false;
        listen = lib.mkForce [
          {
            addr = "127.0.0.1";
            port = 3443;
          }
        ];
      };
      dd.verify.role = pkgs.lib.mkForce "full";
      # the real window: the script never races it, it moves a removal back
      # in the release instead; the gate looks every 2s
      dd.verify.leaveTickSeconds = 2;
      dd.libraries.enable = true;
      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
        pkgs.awscli2
      ];
    };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
