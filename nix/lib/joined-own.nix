# What a joined box is built and judged with: its box.json, with whose it is
# from enlisted.json beside it, which its dd-control writes from the agreed
# state (cmd/dd-control/enlisted.go), and never from box.json's own word. This
# decides only what the box builds: the plan gives it nothing but by the agreed
# state (place.Owners), so a file edited by hand builds more and runs no more
# of it. A box serves its owner's household unless box.json names one.
{ lib }:
dir:
let
  own = lib.importJSON (dir + "/box.json");
  said = dir + "/enlisted.json";
  owner = if builtins.pathExists said then (lib.importJSON said).owner or null else null;
  house = own.house or { };
in
own
// {
  inherit owner;
}
// lib.optionalAttrs (owner != null && (house.household or [ ]) == [ ]) {
  house = house // {
    household = [ owner ];
  };
}
