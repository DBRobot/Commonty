{ config, lib, ... }:
{
  # Passwords: Vaultwarden, signed in through the gate, its files in garage.
  # Its garage key and the sign-in secret it shares with the gate come from
  # sops like every other key here; the box never holds them in the clear.
  imports = [
    ./_sops.nix
    ./_member-mail.nix
    ../modules/vault/vaultwarden.nix
  ];
  dd.home.services = [
    {
      name = "Passwords";
      blurb = "Your passwords, on your devices, and shared with the people you choose.";
      url = "https://vault.${config.dd.domain}/";
      icon = "passwords";
      color = "#1b8aa0";
      rank = 45;
    }
  ];
  sops = lib.mkIf (!(config.dd.failover or false)) {
    secrets = {
      vault-key-id = { };
      vault-key-secret = { };
      vault-oidc-secret = { };
    };
    templates."vault.env".content = ''
      VAULT_ID=${config.sops.placeholder.vault-key-id}
      VAULT_SECRET=${config.sops.placeholder.vault-key-secret}
      AWS_ACCESS_KEY_ID=${config.sops.placeholder.vault-key-id}
      AWS_SECRET_ACCESS_KEY=${config.sops.placeholder.vault-key-secret}
      SSO_CLIENT_SECRET=${config.sops.placeholder.vault-oidc-secret}
    '';
  };
  # its files, copied into this box's own store ahead of failover
  dd.store.migrate.vaultwarden = lib.mkIf (!(config.dd.failover or false)) {
    envFile = config.sops.templates."vault.env".path;
    id = "VAULT_ID";
    secret = "VAULT_SECRET";
  };
  dd.vault = {
    enable = true;
    # with failover on, made on the box (modules/vault/vaultwarden.nix)
    envFile = lib.mkIf (!(config.dd.failover or false)) config.sops.templates."vault.env".path;
    oidcSecretFile = lib.mkIf (
      !(config.dd.failover or false)
    ) config.sops.secrets.vault-oidc-secret.path;
  };
}
