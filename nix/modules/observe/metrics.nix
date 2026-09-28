{
  ddScript,
  config,
  pkgs,
  lib,
  ...
}:
let
  facts = "/var/lib/dd-facts";
in
{
  # Every box keeps its own numbers. Prometheus here scrapes this box and
  # nothing else, and keeps the history on this box's disk. Whoever runs
  # grafana reads each box over the tailnet; nothing aggregates, and a box
  # that is gone takes its own history with it, which is correct. A fleet-wide
  # view, when placement wants one, is Thanos over Garage on top of exactly
  # this, not a change to it.
  #
  # Samples, not events: alerting.nix shouts when something breaks, this
  # notices the gradual things. History cannot be backfilled, so it starts
  # before any dashboard exists.
  services.prometheus = {
    enable = true;
    port = 9090;
    # this box only: thanos's sidecar beside it is how the fleet reads it
    listenAddress = "127.0.0.1";
    retentionTime = "180d"; # a few hundred MB; enough to answer "when did this start"

    exporters.node = {
      enable = true;
      listenAddress = "127.0.0.1";
      port = 9100;
      enabledCollectors = [
        "systemd" # unit states: catches a failed backup even without the mail
        "zfs" # pool state, arc stats, per-dataset space
        "hwmon" # drive and cpu temperatures
        "thermal_zone" # the acpi view of the same, and package throttling
        "powersupplyclass" # a laptop's battery is its ups: charge, health, on ac
        "textfile" # the facts below
      ];
      extraFlags = [ "--collector.textfile.directory=${facts}" ];
    };

    # SMART, read from the drives themselves: health verdict, reallocated
    # and pending sectors, nvme wear and media errors, power-on hours,
    # temperature. smartd already mails when a drive turns; this keeps the
    # slope, which is the part that predicts it.
    exporters.smartctl = {
      enable = true;
      listenAddress = "127.0.0.1";
      port = 9633;
      maxInterval = "5m";
    };

    scrapeConfigs = [
      {
        job_name = "node";
        static_configs = [ { targets = [ "127.0.0.1:9100" ]; } ];
      }
      {
        job_name = "smartctl";
        static_configs = [ { targets = [ "127.0.0.1:9633" ]; } ];
      }
    ]
    # Garage's own figures come from its admin port (storage/garage.nix).
    # 2112 was never garage: on node1 it is ente's museum, which answered
    # as "garage" while garage itself was not measured anywhere.
    ++ lib.optional config.services.garage.enable {
      job_name = "garage";
      static_configs = [ { targets = [ "127.0.0.1:3903" ]; } ];
    }
    ++ lib.optional (config.services.ente.api.enable or false) {
      job_name = "ente-museum";
      static_configs = [ { targets = [ "127.0.0.1:2112" ]; } ];
    };
  };

  # The nvme controller node (/dev/nvme0, not the namespace) is root-only by
  # default, and SMART is read through the controller. The exporter runs in
  # the disk group; let that group at it. Same standing as the sd devices.
  services.udev.extraRules = ''
    KERNEL=="nvme[0-9]*", SUBSYSTEM=="nvme", GROUP="disk", MODE="0660"
  '';

  # What this box is, as metrics next to how it is doing: cores, model,
  # instruction sets, memory, disks, gpu, kernel, generation. Rewritten
  # hourly and at boot, read by node_exporter's textfile collector. The
  # placement program will want the same facts signed by the host key;
  # that is a second output of this same script, later.
  # where every box writes what it knows about itself (this and locate.nix)
  systemd.tmpfiles.rules = [ "d ${facts} 0755 node-exporter node-exporter -" ];

  systemd.services.dd-facts = {
    description = "Publish this box's hardware facts as metrics";
    wantedBy = [ "multi-user.target" ];
    startAt = "hourly";
    path = with pkgs; [
      coreutils
      gawk
      gnugrep
      gnused
      util-linux
      pciutils
    ];
    serviceConfig = {
      Type = "oneshot";

      User = "node-exporter";
      Group = "node-exporter";
    };
    script = ddScript ./facts.sh {
      REGION = config.dd.box.region;
      SITE = config.dd.box.site;
      FACTS = facts;
    };
  };
}
