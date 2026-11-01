# A joined box removes the plaintext on its disk once its owner has ended
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
      description = "Remove the plaintext on this box's disk: its owner %i has ended";
      path = [ pkgs.coreutils ];
      # started by dd-control, never by a switch
      restartIfChanged = false;
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${pkgs.bash}/bin/bash ${./forget-owner.sh} %i";
      };
    };
  };
}
