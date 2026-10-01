# Mail to members, for the whole box: to <name>@<domain>, which Cloudflare
# forwards to the address each member gave the mail Worker. Roles that write
# to members import this.
{ config, ... }:
{
  imports = [
    ./_sops.nix
    ../modules/member-mail.nix
  ];
  sops.secrets.ente-smtp-password = { };
  dd.memberMail.smtpPasswordFile = config.sops.secrets.ente-smtp-password.path;
}
