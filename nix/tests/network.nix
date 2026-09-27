# The fleet's own network on one box: Headscale comes up, the seed makes
# the boxes' user and keys, the box joins its own network as a box and
# names itself there, and the gate hands a member's device a join key in
# that member's name. No relay, no certificate: control is plain http on
# the box's own port, which is what a real box does behind nginx.
{ self, pkgs, ... }:
{
  name = "network";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  nodes.box = {
    imports = [
      ./box.nix
      ../modules/net/headscale.nix
      ../modules/net/box.nix
    ];
    dd.verify.role = pkgs.lib.mkForce "full";
    dd.headscale = {
      enable = true;
      url = "http://127.0.0.1:8085";
      behindNginx = false;
    };
    dd.net.enable = true;
    dd.net.controlAddress = "127.0.0.1";
    # no internet in here: one made-up relay that is never used, from a
    # file, because headscale refuses to start with none
    services.headscale.settings.derp = {
      urls = pkgs.lib.mkForce [ ];
      paths = [
        "${pkgs.writeText "derp-test.yaml" ''
          regions:
            900:
              regionid: 900
              regioncode: test
              regionname: Test
              nodes:
                - name: 900a
                  regionid: 900
                  hostname: box
                  ipv4: 127.0.0.1
                  stunport: 3478
                  derpport: 3479
        ''}"
      ];
    };
    environment.systemPackages = [
      pkgs.curl
      pkgs.jq
      pkgs.tailscale
    ];
  };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
