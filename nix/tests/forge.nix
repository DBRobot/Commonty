# The forge, booted: its setup units make the box's own admin identity and
# protect main. The layout move once left the admin's name unexpanded in
# that setup and no test here booted a forge to notice; now one does. No
# person is its admin: the repository's owner is a member like any other.
{ pkgs, self, ... }:
let
  # a certificate for the box's names, where a real box has its wildcard
  # from acme
  cert = import ../lib/test-cert.nix { inherit pkgs; } {
    subject = "test.invalid";
    names = [ "*.test.invalid" ];
  };
in
{
  name = "forge";
  helpers = true;
  node.specialArgs = { inherit self; };
  nodes.box =
    { config, lib, ... }:
    {
      imports = [
        ./box.nix
        ../modules/forge/forgejo.nix
      ];
      # the full gate: the Git pages' live changes come through it
      dd.verify.role = pkgs.lib.mkForce "full";
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
        pkgs.age
        pkgs.git
      ];
      # nginx in front, as on a box: a run is cancelled through it
      services.nginx.enable = true;
      services.nginx.recommendedProxySettings = true;
      services.nginx.virtualHosts =
        lib.genAttrs (map (h: "${h}.test.invalid") (config.dd.verify.hosts ++ [ "code" ]))
          (_: {
            useACMEHost = lib.mkForce null;
            sslCertificate = "${cert}/cert.pem";
            sslCertificateKey = "${cert}/key.pem";
          });
      # the forge's tile, as roles/forge.nix has it: the demo exists, and is
      # nobody to the forge
      dd.home.services = [
        {
          name = "Code";
          url = "https://git.test.invalid/";
          demo = "full";
          icon = "code";
          color = "#4a5a8a";
        }
      ];
      # a runner box's secrets key, a test's (on a real box, its TPM's): the
      # forge seals its registration tokens to it (runner-keys.nix)
      dd.forgejo.runnerBoxes.box1 = "age1gc25m409smgcklvytkksgew4ycjky4xtwvarsynwjxa9eg60jgpqfrlgp8";
      environment.etc."box1-identity".text =
        "AGE-SECRET-KEY-1RYQV6YMT4DPUURXLGDHLEP6P4QLM4NYQ4Z4E594SKSVYTXHZTF6QAJ94TJ";
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 4096;
    };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
    # the box's own identity (the module's default) and the person who owns
    # the fleet's repository (box.nix: dd.repo)
    admin = "commonty-box";
    # the boxes' identity that stops and reruns CI runs (the module's default)
    ci = "commonty-ci";
    owner = "tester";
    port = 3001; # nothing listens here now; the test checks that
    sock = "/run/forgejo/forgejo.sock";
  };
}
