# Members' own addresses, for the whole box: from sops once someone has put
# them there (`member-emails`, lines of `name: address`), never in the repo
# or on the box's disk. Roles that write to members import this.
{ config, lib, ... }:
let
  hasEmails = lib.hasInfix "\nmember-emails:" (
    "\n" + builtins.readFile (../../secrets + "/${config.networking.hostName}.yaml")
  );
in
{
  imports = [
    ./_sops.nix
    ../modules/member-mail.nix
  ];
  sops.secrets = {
    ente-smtp-password = { };
  }
  // lib.optionalAttrs hasEmails { member-emails = { }; };
  dd.memberMail = {
    smtpPasswordFile = config.sops.secrets.ente-smtp-password.path;
    emailsFile = if hasEmails then config.sops.secrets.member-emails.path else null;
  };
}
