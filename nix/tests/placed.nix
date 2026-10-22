# box/placed.nix on a box: a placed service runs only while its flag is up,
# all of it - its own units, its timers and sockets, a template's
# instances - and a unit two services claim is the closer one's. The box
# starts ungated, as a box did before its services were placed, and
# switches to the release that places them: what ran stops until its flag
# comes, and starts with what starts.json names. The flags are raised by
# hand here, with the units dd-control would start and stop; dd-control's
# own part is placed_test.go's.
{ pkgs, ... }:
let
  long = {
    serviceConfig.ExecStart = "${pkgs.coreutils}/bin/sleep infinity";
  };
  boot = long // {
    wantedBy = [ "multi-user.target" ];
  };
in
{
  name = "placed";
  helpers = true;
  nodes.box =
    { lib, ... }:
    {
      imports = [
        ./box.nix
        ../modules/box/placed.nix
      ];
      options.dd.control.enable = lib.mkEnableOption "placement";
      config = {
        system.switch.enable = true;
        dd.verify.enable = false;
        dd.failover = true;
        # what the release gives it to carry (fleet-placement.nix carries):
        # not pihole, whose units it is built with all the same
        dd.placed.services = [
          "dd-games"
          "forgejo-runner"
          "members-runner"
          "thanos-store"
        ];
        # units by the names the needs give them (nix/lib/needs.nix)
        systemd.services = {
          # dd-games: its own, a template's instances, a timer's run
          dd-games = boot;
          "dd-game@" = long;
          games-access = {
            serviceConfig.Type = "oneshot";
            script = "true";
          };
          # forgejo-runner claims gitea-runner-*, members-runner the
          # closer gitea-runner-members*
          gitea-runner-box = boot;
          gitea-runner-members-box = boot;
          # thanos-store: placed, and carried
          thanos-store = boot;
          # pihole: placed, built in and not carried
          pihole-ftl = boot;
          # zfs-datasets runs on every box: never switched
          dd-snapshot-trim = boot;
        };
        systemd.timers.games-access = {
          wantedBy = [ "timers.target" ];
          timerConfig.OnUnitActiveSec = "1h";
          timerConfig.OnActiveSec = "1h";
        };
        systemd.sockets.tangd = {
          wantedBy = [ "sockets.target" ];
          listenStreams = [ "127.0.0.1:5000" ];
          socketConfig.Accept = true;
        };
        systemd.services."tangd@" = long;
        dd.control.enable = lib.mkDefault false;
        # the release that places them
        specialisation.placed.configuration.dd.control.enable = true;
      };
    };
}
