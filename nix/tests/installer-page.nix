# The box installer's screen script, under node: each step asks its
# question on the way in (the disk step once never did).
{ pkgs, ... }:
pkgs.runCommandLocal "installer-page"
  {
    nativeBuildInputs = [ pkgs.nodejs ];
  }
  ''
    node ${./installer-page/steps.test.mjs} ${../installer/web/setup.js}
    touch $out
  ''
