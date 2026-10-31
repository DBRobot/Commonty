# A joined box removes what is on its disk of a member it no longer serves
# (forget-owner.sh). Only a joined box has this unit at all: a box of the
# release has nothing that could start it, or anything for it to remove.
{
  config,
  lib,
  pkgs,
  ...
}:
{
  config = lib.mkIf config.dd.box.joined {
    systemd.services."dd-forget-owner@" = {
      description = "Remove what is on this box's disk of its former owner %i";
      path = [
        pkgs.coreutils
        pkgs.jq
      ];
      # started by dd-control, never by a switch
      restartIfChanged = false;
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${pkgs.bash}/bin/bash ${./forget-owner.sh} %i";
      };
    };
  };
}
