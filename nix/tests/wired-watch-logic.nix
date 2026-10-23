# The wired watchdog's every choice, without a VM: box/wired-watch.sh run by
# run on a made-up box (wired-watch-logic/run.sh). The fallback is only ever
# to a wifi the router answers on, and the cable is back up whenever the
# wifi goes too. The VM test (wired-watch) has the real links.
{ pkgs, ... }:
pkgs.runCommand "wired-watch-logic"
  {
    nativeBuildInputs = [
      pkgs.coreutils
      pkgs.gnugrep
      pkgs.gnused
    ];
  }
  ''
    bash ${./wired-watch-logic/run.sh} ${../modules/box/wired-watch.sh}
    touch $out
  ''
