# Passwords on one box: Vaultwarden on postgres with its files in the box's
# own store, signed in only through the gate's issuer, and the Send page
# anyone with a link may open. As with failover on (vault/vaultwarden.nix
# own): its store key and sign-in secret are made on the box, not sops'.
{ pkgs, self, ... }:
let
  # a test key only: on a real box this is its TPM-held secrets key
  identity = "AGE-SECRET-KEY-1RYQV6YMT4DPUURXLGDHLEP6P4QLM4NYQ4Z4E594SKSVYTXHZTF6QAJ94TJ";
in
{
  name = "vault";
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
        ../modules/vault/vaultwarden.nix
      ];
      dd.failover = true;
      dd.verify.role = lib.mkForce "full";
      dd.store = {
        enable = true;
        capacity = "1G";
        identity = "/etc/age-identity";
        agePlugins = [ ];
      };
      environment.etc."age-identity" = {
        text = identity;
        mode = "0400";
      };
      # no certificate in a test
      services.nginx.virtualHosts."store.box.test.invalid" = {
        useACMEHost = lib.mkForce null;
        forceSSL = lib.mkForce false;
      };
      dd.vault = {
        enable = true;
        # no tls in here: Vaultwarden asks the gate directly
        issuer = "http://127.0.0.1:4181/_dd/oidc";
        pwnedDir = "/srv/pwned";
      };
      services.postgresqlBackup.enable = true;
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
