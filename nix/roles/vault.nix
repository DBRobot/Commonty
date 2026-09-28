{ config, lib, ... }:
let
  # each member's own address, only ever in sops: `name: address` lines.
  # Until someone adds them no mail leaves (see modules/vault).
  hasEmails = lib.hasInfix "\nmember-emails:" (
    "\n" + builtins.readFile (../../secrets + "/${config.networking.hostName}.yaml")
  );
in
{
  # Passwords: Vaultwarden, signed in through the gate, its files in garage.
  # Its garage key and the sign-in secret it shares with the gate come from
  # sops like every other key here; the box never holds them in the clear.
  imports = [
    ./_sops.nix
    ../modules/vault/vaultwarden.nix
  ];
  dd.home.services = [
    {
      name = "Passwords";
      blurb = "Your passwords, on your devices, and shared with the people you choose.";
      url = "https://vault.${config.dd.domain}/";
      icon = "passwords";
      color = "#2f7a5f";
      rank = 45;
    }
  ];
  sops.secrets = {
    vault-key-id = { };
    vault-key-secret = { };
    vault-oidc-secret = { };
    ente-smtp-password = { };
  }
  // lib.optionalAttrs hasEmails { member-emails = { }; };
  sops.templates."vault.env".content = ''
    VAULT_ID=${config.sops.placeholder.vault-key-id}
    VAULT_SECRET=${config.sops.placeholder.vault-key-secret}
    AWS_ACCESS_KEY_ID=${config.sops.placeholder.vault-key-id}
    AWS_SECRET_ACCESS_KEY=${config.sops.placeholder.vault-key-secret}
    SSO_CLIENT_SECRET=${config.sops.placeholder.vault-oidc-secret}
  '';
  dd.vault = {
    enable = true;
    envFile = config.sops.templates."vault.env".path;
    oidcSecretFile = config.sops.secrets.vault-oidc-secret.path;
    smtpPasswordFile = config.sops.secrets.ente-smtp-password.path;
    memberEmails = if hasEmails then config.sops.secrets.member-emails.path else null;
  };
}
