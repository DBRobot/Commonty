# A box's room for computers, without a VM: computers/computer-facts.sh on
# a made-up box (computer-facts-logic/run.sh). The services' measured
# memory gives the room, the needs' sizes whenever there is no fresh
# measure, and never below nothing. The computers VM test runs it for real.
{ pkgs, ... }:
pkgs.runCommand "computer-facts-logic"
  {
    nativeBuildInputs = [
      pkgs.coreutils
      pkgs.gawk
      pkgs.gnugrep
      pkgs.findutils
    ];
  }
  ''
    bash ${./computer-facts-logic/run.sh} ${../modules/computers/computer-facts.sh}
    touch $out
  ''
