# The home controller (docs/dev/home-controller.md, nix/controller): its card
# locked to the house box's Tang and to nothing else (a spare disk stands in
# for the card's free space; lock.sh is the script the first boot runs), and
# the door in front of Home Assistant: paired with the card's key, a
# member's device signing in, a stranger's not, each person their own Home
# Assistant user. The door answers plain http here: its certificate comes
# from the DNS Worker, which has its own check.
{ pkgs, self, ... }:
{
  name = "home-controller";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1024;
  # the house's box: Tang, for the house network alone (vlan 1); vlan 2 is
  # somewhere else
  nodes.house = {
    imports = [ ../modules/smarthome/tang.nix ];
    virtualisation.vlans = [
      1
      2
    ];
    dd.tang = {
      enable = true;
      house = "192.168.1.0/24";
    };
    networking.firewall.enable = true;
  };
  # a Tang someone else stood up on the house network
  nodes.stranger = {
    virtualisation.vlans = [ 1 ];
    services.tang = {
      enable = true;
      listenStream = [ "7654" ];
      ipAddressAllow = [
        "192.168.1.0/24"
        "127.0.0.1/32"
      ];
    };
    networking.firewall.allowedTCPPorts = [ 7654 ];
  };
  nodes.controller = {
    imports = [
      ../modules/box/domain.nix
      ../modules/smarthome/home-assistant.nix
      ../modules/smarthome/door.nix
    ];
    virtualisation.vlans = [ 1 ];
    virtualisation.memorySize = pkgs.lib.mkForce 2048;
    virtualisation.emptyDiskImages = [ 256 ];
    dd.domain = "commonty.test";
    dd.repo = "test/test";
    dd.smarthome.enable = true;
    dd.door = {
      enable = true;
      pairKey = "/var/lib/test-pair";
    };
    systemd.services.dd-door.environment = {
      # the made-up member is the test's fleet (client/mail/tests/tester.json)
      DOOR_MEMBERS = pkgs.lib.mkForce "${pkgs.runCommand "test-members.json"
        { nativeBuildInputs = [ pkgs.jq ]; }
        ''
          jq -r .members ${../../client/mail/tests/tester.json} > $out
        ''
      }";
      DOOR_PLAIN_HTTP = "true";
      # the gate's controller routes, stood in for on the controller itself
      DOOR_GATE = pkgs.lib.mkForce "http://127.0.0.1:9000";
      DOOR_BACKUP_RETRY = "20";
      DOOR_LISTEN = pkgs.lib.mkForce "0.0.0.0:80";
    };
    systemd.services.test-gate = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        ExecStart = "${pkgs.python3}/bin/python3 ${./home-controller/gate.py}";
        StateDirectory = "test-gate";
      };
    };
    # the card's pairing key, as the installer writes it
    systemd.tmpfiles.rules = [
      "f /var/lib/test-pair 0600 root root - 0123456789abcdef0123456789abcdef"
    ];
    boot.initrd.kernelModules = [ "dm_crypt" ];
    environment.systemPackages = [
      pkgs.clevis
      pkgs.jose
      pkgs.curl
      pkgs.cryptsetup
      pkgs.jq
      pkgs.e2fsprogs
      pkgs.util-linux
      (pkgs.python3.withPackages (ps: [
        ps.cryptography
        ps.pynacl
      ]))
    ];
  };
  # the Pi taken out of the house
  nodes.away = {
    virtualisation.vlans = [ 2 ];
    environment.systemPackages = [ pkgs.curl ];
  };
  scriptEnv = {
    lock = "${../controller/lock.sh}";
    client = "${./home-controller/client.py}";
    openBackup = "${./home-controller/open_backup.py}";
    tester = "${../../client/mail/tests/tester.json}";
  };
}
