{
  config,
  pkgs,
  lib,
  utils,
  ddScript,
  ...
}:
let
  units = config.dd.runnerRestart.units;
  script = pkgs.writeShellScript "runner-restart" (
    ddScript ./runner-restart.sh {
      PATH = lib.makeBinPath (
        [
          config.systemd.package
          pkgs.coreutils
          pkgs.gnugrep
        ]
        ++ lib.optional config.virtualisation.docker.enable config.virtualisation.docker.package
      );
      FACTS = "/var/lib/dd-facts";
    }
  );
  hash = text: if text == "" then "" else builtins.hashString "sha256" text;
  entry =
    unit: u:
    lib.concatStringsSep "|" [
      "${unit}.service"
      u.job
      (toString u.label)
      (toString u.limit)
      (hash u.sandbox)
      (lib.concatMapStringsSep "," (d: "${d}.service") u.drain)
    ];
  # unit names carry systemd's escapes (members\x2d<host>), which an Exec
  # line would unescape
  exec = args: utils.escapeSystemdExecArgs ([ script ] ++ args);
in
{
  # A switch never stops a unit that holds CI jobs: a release that changed
  # the runner stopped it under the next pull request's CI and failed the
  # run (release 331, run 786). Its new definition waits until it holds no
  # job, and dd-runner-restart restarts it then.
  # The wait has a limit, though: a member who kept a job running back to
  # back put off a new sandbox for good. Past it the unit drains (its runner
  # takes no new job, and the jobs under way get its shutdown timeout) and
  # restarts. A change to the sandbox itself does not wait for idle at all.
  options.dd.runnerRestart.units = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule {
        options = {
          job = lib.mkOption {
            type = lib.types.enum [
              "cgroup"
              "docker"
            ];
            description = "What a job in it is: processes in its cgroup besides its main one, or a running container carrying `label`";
          };
          label = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            description = "docker: the label (key=value) on the containers that are its jobs; others on the box do not count";
          };
          limit = lib.mkOption {
            type = lib.types.ints.positive;
            description = "Seconds a new definition waits for no job; then the unit drains and restarts all the same";
          };
          drain = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ ];
            description = "Units (no .service) stopped, each a drain, before this one restarts past its limit or for a sandbox change, and started again after";
          };
          sandbox = lib.mkOption {
            type = lib.types.str;
            default = "";
            description = "What of its config is the sandbox, as text. A change there drains and restarts it on the next pass, cancelling the jobs under way";
          };
        };
      }
    );
    default = { };
    description = "Units a switch leaves running, by name (no .service)";
  };

  config = lib.mkIf (units != { }) {
    assertions = lib.mapAttrsToList (unit: u: {
      assertion = u.job != "docker" || u.label != null;
      message = "dd.runnerRestart.units.${unit}: a docker unit's jobs are the containers with its label";
    }) units;
    systemd.services =
      lib.mapAttrs (unit: u: {
        restartIfChanged = false;
        # as root: which unit definition this run started from
        serviceConfig.ExecStartPre = lib.mkBefore [
          "+${
            exec [
              "started"
              "${unit}.service"
              (hash u.sandbox)
            ]
          }"
        ];
      }) units
      // {
        # every minute: each unit whose definition changed and holds no job
        # restarts into it. A job taken in the instant between is drained,
        # not killed (the runners' shutdown_timeout). How long each has
        # waited goes to the box's facts (dd_runner_restart_pending_seconds),
        # and fleet-alerts mails one over an hour.
        dd-runner-restart = {
          description = "Restart the runners into their new config once they hold no job";
          restartIfChanged = false;
          serviceConfig = {
            Type = "oneshot";
            ExecStart = exec ([ "check" ] ++ lib.mapAttrsToList entry units);
          };
        };
      };
    systemd.timers.dd-runner-restart = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1min";
        OnUnitActiveSec = "1min";
      };
    };
  };
}
