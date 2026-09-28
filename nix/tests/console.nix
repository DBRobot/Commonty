# The way in at the keyboard. A box that loses its network still has a
# monitor and a keyboard, and until now it had nothing else: admin was
# made with no password and kept it, because nixos writes a declarative
# password only for a user it is creating unless users are immutable.
# On a real box the hash comes from sops before users are made
# (roles/core.nix); what this proves is the half that was wrong, which is
# that a hashedPasswordFile reaches /etc/shadow at all.
{ pkgs, ... }:
{
  name = "console";
  nodes.box =
    { ... }:
    {
      users.mutableUsers = false;
      users.users.admin = {
        isNormalUser = true;
        extraGroups = [ "wheel" ];
        hashedPasswordFile = toString (
          pkgs.writeText "console-hash" "$6$cnsltest$TRJuIaBEltCG2Yexl/gXIHaRnqsn62Ednu1JMGl4GZTK46eSS6sCT93nmyVM6Pd.fT6DAbZ75y4YZXkeUReEb0\n"
        );
      };
      security.sudo.wheelNeedsPassword = false; # as roles/core.nix has it
    };
  testScript = builtins.readFile ./console.py;
}
