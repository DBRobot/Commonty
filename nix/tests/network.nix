# The fleet's own network on one box: Headscale comes up, the seed makes
# the boxes' user and keys, the box joins its own network as a box and
# names itself there, and the gate hands a member's device a join key in
# that member's name. No relay, no certificate: control is plain http on
# the box's own port, which is what a real box does behind nginx.
{ self, pkgs, ... }:
{
  name = "network";
  # nix/tests/lib's waits: one run's journal (journal_of)
  helpers = true;
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  nodes.box = {
    imports = [
      ./box.nix
      ../modules/net/headscale.nix
      ../modules/net/box.nix
      ../modules/net/owner.nix
    ];
    # the admin's and the host's ports are the firewall's to keep
    networking.firewall.enable = pkgs.lib.mkForce true;
    # 8085 headscale; 4182 the gate as a real box serves it, behind its
    # front door; 443 the front door an installer may reach on the network
    networking.firewall.allowedTCPPorts = [
      8085
      4182
      443
    ];
    services.headscale.address = pkgs.lib.mkForce "0.0.0.0";
    services.openssh.enable = true;
    # kim has the box at home; sarah is the fleet's admin. A host is not an
    # admin: kim's devices get its dns and nothing more
    dd.box.owner = "kim";
    dd.verify.admin = "sarah";
    dd.verify.role = pkgs.lib.mkForce "full";
    dd.headscale = {
      enable = true;
      url = "http://box:8085";
      behindNginx = false;
      # the rules headscale has to load: the admin's, and the host's
      admin = "sarah";
      owners."100.64.0.1" = "kim";
    };
    dd.net.enable = true;
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
      pkgs.sqlite
      pkgs.openssl
      pkgs.age
      pkgs.socat
      # the stand-in for dd-control (network/control.py)
      pkgs.python3
    ];
  };
  # the owner's laptop and another member's, each on the network as a person
  nodes.laptop = {
    services.tailscale.enable = true;
    virtualisation.memorySize = pkgs.lib.mkForce 1024;
    # the test network only: dhcp on the qemu nic hands out a default route
    # mid-join, and tailscaled drops its control connection on that change
    networking.useDHCP = false;
    environment.systemPackages = [
      pkgs.netcat
      pkgs.jq
    ];
  };
  nodes.other = {
    services.tailscale.enable = true;
    virtualisation.memorySize = pkgs.lib.mkForce 1024;
    # the test network only: dhcp on the qemu nic hands out a default route
    # mid-join, and tailscaled drops its control connection on that change
    networking.useDHCP = false;
    environment.systemPackages = [
      pkgs.netcat
      pkgs.jq
      # last, a box installer on the network for its backup alone
      (import ../installer/network.nix { inherit pkgs self; })
      self.packages.${pkgs.stdenv.hostPlatform.system}.agent
      pkgs.age
      pkgs.curl
    ];
  };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
    control = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd-control}/bin/dd-control";
    standin = ./network/control.py;
  };
}
