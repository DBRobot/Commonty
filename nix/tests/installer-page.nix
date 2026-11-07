# The box installer's screen script, under node: each step asks its
# question on the way in (the disk step once never did), and the app's
# set-up-a-box page writes a second drive (it once wrote only the first).
{ pkgs, ... }:
pkgs.runCommandLocal "installer-page"
  {
    nativeBuildInputs = [ pkgs.nodejs ];
  }
  ''
    node ${./installer-page/steps.test.mjs} ${../installer/web/setup.js}
    node ${./installer-page/boxsetup.test.mjs} ${../../app/web/boxsetup.js}
    touch $out
  ''
