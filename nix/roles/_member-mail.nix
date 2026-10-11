# Mail to members, for the whole box: to <name>@<domain>, through the mail
# Worker, which sends it to the address each member gave it. Roles that
# write to members import this.
{
  imports = [ ../modules/member-mail.nix ];
}
