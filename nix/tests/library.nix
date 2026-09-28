# A library through the gate, driven by rclone itself: a box with garage
# and the full verifier, a member made on it, a library in their entry, and
# rclone as a crypt remote over the gate's WebDAV copying files in, listing
# them, reading them back, and deleting one (which lands in the trash).
# The bucket holds ciphertext under names it cannot read.
{ pkgs, self, ... }:
let
  rpc = pkgs.writeText "garage.env" "GARAGE_RPC_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
  libraryKey = pkgs.writeText "library-key.env" ''
    LIBRARY_ID=GK0123456789abcdef01234567
    LIBRARY_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  '';
in
{
  name = "library";
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
    dd.garage = {
      zone = "r-test";
      capacity = "1G";
      dataDir = "/srv/garage";
      publicAddr = "box:3901";
      envFile = rpc;
      replicationFactor = 1; # one box in here
    };
    dd.libraries = {
      enable = true;
      keyFile = libraryKey;
    };
    # the gate reaches the bucket on this box directly (no nginx in here)
    dd.verify.library.s3 = pkgs.lib.mkForce "http://127.0.0.1:3900";
    # a tile the demo may use is what makes the demo account exist at all
    dd.home.services = [
      {
        name = "Files";
        url = "https://files.test.invalid/_dd/files";
        icon = "files";
        color = "#2f6fd6";
        demo = "read";
      }
    ];
    environment.systemPackages = [
      pkgs.curl
      pkgs.jq
      pkgs.rclone
      pkgs.awscli2
    ];
  };
  scriptEnv = {
    inherit rpc;
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
