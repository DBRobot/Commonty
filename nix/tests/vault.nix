# Passwords on one box: Vaultwarden on postgres with its files in garage,
# signed in only through the gate's issuer, and the Send page anyone with a
# link may open. The keys are test values in the store, where on a real box
# they come from sops (roles/vault.nix).
{ pkgs, self, ... }:
let
  rpc = pkgs.writeText "garage.env" "GARAGE_RPC_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
  vaultEnv = pkgs.writeText "vault.env" ''
    VAULT_ID=GK0123456789abcdef01234567
    VAULT_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    AWS_ACCESS_KEY_ID=GK0123456789abcdef01234567
    AWS_SECRET_ACCESS_KEY=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    SSO_CLIENT_SECRET=test-oidc-secret
  '';
in
{
  name = "vault";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  defaults.virtualisation.diskSize = 4096;
  nodes.box = {
    imports = [
      ./box.nix
      ../modules/storage/garage.nix
      ../modules/vault/vaultwarden.nix
    ];
    dd.verify.role = pkgs.lib.mkForce "full";
    dd.garage = {
      zone = "r-test";
      capacity = "1G";
      dataDir = "/srv/garage";
      publicAddr = "box:3901";
      envFile = rpc;
      replicationFactor = 1;
    };
    dd.vault = {
      enable = true;
      envFile = vaultEnv;
      oidcSecretFile = pkgs.writeText "oidc" "test-oidc-secret";
      pwnedDir = "/srv/pwned";
    };
    dd.memberMail = {
      smtpPasswordFile = pkgs.writeText "smtp" "unused";
      emailsFile = pkgs.writeText "emails" "sarah: sarah@example.net\n";
    };
    services.postgresqlBackup.enable = true;
    environment.systemPackages = [
      pkgs.curl
      pkgs.jq
      pkgs.awscli2
    ];
  };
  scriptEnv = {
    inherit rpc;
  };
}
