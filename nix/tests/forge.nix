# The forge, booted: its setup units make the admin and protect main. The
# layout move once left the admin's name unexpanded in that setup and no
# test here booted a forge to notice; now one does.
{ pkgs, self, ... }:
let
  # a certificate for the box's names, where a real box has its wildcard
  # from acme
  cert = pkgs.runCommand "test-cert" { nativeBuildInputs = [ pkgs.openssl ]; } ''
    mkdir $out
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
      -subj '/CN=test.invalid' -addext 'subjectAltName=DNS:*.test.invalid' \
      -keyout $out/key.pem -out $out/cert.pem
  '';
in
{
  name = "forge";
  node.specialArgs = { inherit self; };
  nodes.box =
    { config, lib, ... }:
    {
      imports = [
        ./box.nix
        ../modules/forge/forgejo.nix
      ];
      dd.forgejo.admin = "tester";
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
