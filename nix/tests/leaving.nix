# Leaving (docs/dev/leaving.md): a member leaves with their root's signed
# word, every way in ends at once and the gate deletes their libraries, disk
# images, notes and names straight away; a member the owner removes keeps
# everything for the (shortened) window, then loses it the same way; another
# member's things are never touched.
{ pkgs, self, ... }:
let
  rpc = pkgs.writeText "garage.env" "GARAGE_RPC_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
  libraryKey = pkgs.writeText "library-key.env" ''
    LIBRARY_ID=GK0123456789abcdef01234567
    LIBRARY_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  '';
in
{
  name = "leaving";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  defaults.virtualisation.diskSize = 4096;
  nodes.box = {
    imports = [
      ./box.nix
      ../modules/storage/garage.nix
      ../modules/library/libraries.nix
    ];
    dd.verify.role = pkgs.lib.mkForce "full";
    # a removal keeps things a minute, not 30 days, and the gate looks
    # every 2s: the script's checks inside the window take ~20s at most
    dd.verify.leaveWindowSeconds = 60;
    dd.verify.leaveTickSeconds = 2;
    dd.garage = {
      zone = "r-test";
      capacity = "1G";
      dataDir = "/srv/garage";
      publicAddr = "box:3901";
      envFile = rpc;
      replicationFactor = 1;
    };
    dd.libraries = {
      enable = true;
      keyFile = libraryKey;
    };
    dd.verify.library.s3 = pkgs.lib.mkForce "http://127.0.0.1:3900";
    environment.systemPackages = [
      pkgs.curl
      pkgs.jq
      pkgs.awscli2
    ];
  };
  scriptEnv = {
    inherit rpc;
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
