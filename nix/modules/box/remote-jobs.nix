# Jobs whose work is a call to another machine (a certificate's renewal, a
# list fetched from the internet), kept out of every switch. A switch waits
# on each job it starts, and a oneshot has no start timeout: node2's switch
# to release 358 sat ten minutes on acme-order-renew while the DNS Worker
# and Let's Encrypt took their time (2026-10-22). So such a job is never
# started by a switch, nor wanted by anything the switch starts. Its own
# timer starts it a little after boot, and again a little after any switch
# that changed it, as a job of its own. Its failure is the other machine's,
# not the release's: the agent leaves it out when it judges a switch
# (/etc/dd/remote-jobs, dd-agent's failed_units).
{ config, lib, ... }:
let
  jobs = config.dd.remoteJobs;
in
{
  options.dd.remoteJobs = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule {
        options = {
          delay = lib.mkOption {
            type = lib.types.str;
            default = "1min";
            description = "How long after boot, or after a switch that changed the job, it starts";
          };
          timeout = lib.mkOption {
            type = lib.types.str;
            default = "15min";
            description = "How long one run may take before systemd ends it; the job's own timer tries again";
          };
        };
      }
    );
    default = { };
    description = "Services (by name, no .service) whose work is another machine's answer: never part of a switch, and never held against one";
  };

  config = {
    systemd.services = lib.mapAttrs (_: j: {
      wantedBy = lib.mkForce [ ];
      restartIfChanged = false;
      serviceConfig.TimeoutStartSec = j.timeout;
    }) jobs;
    systemd.timers = lib.mapAttrs' (
      u: j:
      lib.nameValuePair "${u}-after-switch" {
        wantedBy = [ "timers.target" ];
        # the job's new definition restarts this timer in the switch, and
        # the timer starts the job after it, on its own
        restartTriggers = [ config.systemd.units."${u}.service".unit ];
        timerConfig = {
          OnActiveSec = j.delay;
          Unit = "${u}.service";
        };
      }
    ) jobs;
    environment.etc."dd/remote-jobs".text = lib.concatMapStrings (u: "${u}.service\n") (
      lib.attrNames jobs
    );
  };
}
