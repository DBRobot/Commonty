# The condition a placed service's units start on besides its flag: not
# while dd-control hands it over (servers/system/control/cmd/dd-control/
# placed.go marks the flag "handing":true), so nothing writes while or after
# its last writes are copied out. box/placed.nix gives it to the units; a
# test that raises flags by hand gives it the same way.
{ pkgs }:
svc:
pkgs.writeShellScript "dd-placed-not-handing-${svc}" ''
  if ${pkgs.gnugrep}/bin/grep -qs '"handing":true' /run/dd-placed/${svc}; then
    echo "${svc} is being handed over: not started"
    exit 1
  fi
''
