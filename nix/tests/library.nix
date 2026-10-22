# A library through the gate, driven by rclone itself: a box with its own
# store and the full verifier, a member made on it, a library in their entry, and
# rclone as a crypt remote over the gate's WebDAV copying files in, listing
# them, reading them back, and deleting one (which lands in the trash).
# The bucket holds ciphertext under names it cannot read.
{ pkgs, self, ... }:
let
  # the old pool's key: only what a copy from it would read with; the gate's
  # keys are the store's, made on the box
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
  nodes.box =
    { lib, ... }:
    {
      imports = [
        ./box.nix
        ../modules/storage/store.nix
        ../modules/library/libraries.nix
      ];
      dd.verify.role = pkgs.lib.mkForce "full";
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
      dd.libraries = {
        enable = true;
        keyFile = libraryKey;
      };
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
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
