# Photos without Ente's web apps: museum as the module builds it (patched,
# on loopback, sessions kept as hashes) on postgres, and the gate's side of
# the sign-in - the page, its settings, and the fleet's verification code
# filled in for a member's own address and nobody else's. The web apps are
# left out: building them is most of an hour, and what they do in a browser
# is checked by hand (docs/dev/packaged.md). Secrets are test values in the
# store, where on a real box they come from sops (roles/photos.nix).
{
  pkgs,
  lib,
  self,
  ...
}:
let
  # museum reads its keys as base64: zeros are as good as any here
  secrets = pkgs.runCommand "photos-test-secrets" { } ''
    mkdir $out
    echo 123456 > $out/ente-ott
    head -c 32 /dev/zero | base64 -w0 > $out/ente-key-encryption
    head -c 64 /dev/zero | base64 -w0 > $out/ente-key-hash
    head -c 32 /dev/zero | base64 -w0 > $out/ente-jwt-secret
    echo GK0123456789abcdef01234567 > $out/garage-key-id
    echo unused > $out/garage-key-secret
    echo unused > $out/ente-smtp-password
  '';
  cert = pkgs.runCommand "test-cert" { nativeBuildInputs = [ pkgs.openssl ]; } ''
    mkdir $out
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
      -subj '/CN=test.invalid' -addext 'subjectAltName=DNS:*.test.invalid' \
      -keyout $out/key.pem -out $out/cert.pem
  '';
in
{
  name = "photos";
  node.specialArgs = { inherit self; };
  # ente.nix pins museum by an overlay, which a test's shared, read-only
  # nixpkgs would refuse
  node.pkgsReadOnly = false;
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  defaults.virtualisation.diskSize = 2048;
  nodes.box =
    { config, ... }:
    {
      imports = [
        ./box.nix
        ../modules/photos/ente.nix
        # what the module reads of sops: each secret's path, here a file in
        # the store
        {
          options.sops.secrets = lib.mkOption {
            type = lib.types.attrsOf (
              lib.types.submodule (
                { name, ... }:
                {
                  options.path = lib.mkOption {
                    type = lib.types.str;
                    default = "${secrets}/${name}";
                  };
                }
              )
            );
          };
        }
      ];
      sops.secrets = lib.genAttrs [
        "ente-ott"
        "ente-key-encryption"
        "ente-key-hash"
        "ente-jwt-secret"
        "garage-key-id"
        "garage-key-secret"
        "ente-smtp-password"
      ] (_: { });
      dd.verify.role = lib.mkForce "full";
      dd.verify.photos = {
        # museum itself: no tls in here
        api = "http://127.0.0.1:8080";
        emailSuffix = "@users.test.invalid";
        codeFile = "${secrets}/ente-ott";
      };
      dd.photos.admin = 1;
      services.ente.web.enable = lib.mkForce false;
      # what the web module sets for museum when it is on
      services.ente.api.settings = {
        apps = {
          accounts = "https://accounts.test.invalid";
          cast = "https://cast.test.invalid";
          public-albums = "https://albums.test.invalid";
        };
        webauthn = {
          rpid = "accounts.test.invalid";
          rporigins = [ "https://accounts.test.invalid" ];
        };
      };
      services.nginx.virtualHosts = lib.mkMerge [
        (lib.genAttrs
          (map (h: "${h}.test.invalid") (
            lib.unique (
              config.dd.verify.hosts
              ++ [
                "api"
                "accounts"
                "albums"
                "cast"
                "photos"
                "locker"
              ]
            )
          ))
          (_: {
            useACMEHost = lib.mkForce null;
            sslCertificate = "${cert}/cert.pem";
            sslCertificateKey = "${cert}/key.pem";
          })
        )
        {
          # Locker is one of the web apps left out
          "locker.test.invalid".locations."/".root = lib.mkForce "/var/empty";
        }
      ];
      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
      ];
    };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
