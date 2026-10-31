# What a member's device may reach for games on a box built from the release's
# defaults, whichever box that is (a box that joined has no way to name other
# ports): the pool, and every port a catalogue game insists on.
{ lib }:
let
  games = (lib.importJSON ../../data/games/catalogue.json).games;
  portBase = 27000;
  portCount = 200;
  fixedPorts = lib.unique (
    lib.concatMap (
      g:
      let
        fixed = g.fixed_ports or { };
      in
      lib.attrValues fixed
      ++ lib.optionals (fixed ? SERVER_PORT) (
        map (o: fixed.SERVER_PORT + o) (lib.attrValues (g.implicit_ports or { }))
      )
    ) games
  );
in
{
  inherit portBase portCount fixedPorts;
  reach = [
    "${toString portBase}-${toString (portBase + portCount - 1)}"
  ]
  ++ map toString fixedPorts;
}
