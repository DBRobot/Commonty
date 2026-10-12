# What watching the fleet needs of a box (types: nix/lib/needs-type.nix)
{
  prometheus = {
    what = "this box's own metrics, kept here until the sidecar has them in the bucket";
    everyBox = true;
    base = {
      # a few weeks of one box's series, and room for a compaction
      requires.disk.size.min = "4G";
      # node1, node2, 2026-10-04: peak; its data
      uses = {
        memory = "136M";
        storage = "109M";
      };
      network = {
        outbound = "none";
        inbound = "localhost";
      };
      depends.prometheus-node-exporter.local = true;
      privacy = "metadata"; # how the box is doing; no member's figures (storage/usage.nix)
    };
  };

  prometheus-node-exporter = {
    what = "the box's own counters, and the facts this repo writes beside them (/var/lib/dd-facts)";
    units = [ "dd-facts" ];
    everyBox = true;
    base = {
      # node1, 2026-10-04: peak
      uses = {
        memory = "23M";
        storage = "0";
      };
      network = {
        outbound = "none";
        inbound = "localhost";
      };
      depends = { };
      privacy = "metadata";
    };
  };

  thanos-sidecar = {
    what = "puts this box's metric blocks in the bucket, where the fleet view reads them";
    everyBox = true;
    base = {
      # node2, 2026-10-04: peak
      uses = {
        memory = "34M";
        storage = "0";
      };
      network = {
        outbound = [ "s3" ];
        inbound = "localhost"; # this box's own query only
      };
      depends = {
        prometheus.local = true;
        garage.local = false;
      };
      privacy = "metadata";
    };
  };

  thanos-store = {
    what = "every box's history, read out of the bucket";
    base = {
      # node1, 2026-10-04: peak; its index cache
      uses = {
        memory = "24M";
        storage = "3M";
      };
      network = {
        outbound = [ "s3" ];
        inbound = "localhost";
      };
      depends.garage.local = false;
      privacy = "metadata";
    };
  };

  thanos-query = {
    what = "the fleet view: this box's own sidecar and the bucket, as one";
    alwaysOn = true;
    base = {
      # node1, 2026-10-04: peak
      uses = {
        memory = "15M";
        storage = "0";
      };
      network = {
        outbound = "none";
        inbound = "localhost";
      };
      depends = {
        thanos-store.local = true;
        thanos-sidecar.local = true;
      };
      privacy = "metadata";
    };
  };

  joined-metrics = {
    what = "the metrics the boxes members set up send, each box apart, for its member alone";
    units = [
      "thanos-receive-joined"
      "thanos-query-joined"
      "dd-metrics-joined"
      # on a joined box: its token to send them with
      "dd-metrics-key"
    ];
    instances = 1;
    base = {
      uses = {
        memory = "64M";
        storage = "1G";
        estimated = true; # a receive and a query; one joined box's share each
      };
      network = {
        outbound = "none";
        inbound = "network"; # the joined boxes, on the store's port
      };
      depends.nginx.local = true;
      privacy = "metadata";
    };
  };

  thanos-compact = {
    what = "merges and thins the history in the bucket";
    # two at once would fight over the same blocks
    instances = 1;
    base = {
      # it downloads blocks to merge them: room for the biggest, overestimated
      requires.disk.size.min = "10G";
      # node1, 2026-10-04: peak
      uses = {
        memory = "117M";
        storage = "1M";
      };
      network = {
        outbound = [ "s3" ];
        inbound = "localhost";
      };
      depends.garage.local = false;
      privacy = "metadata";
    };
  };

  metrics-page = {
    what = "metrics.<domain>: our pages over the fleet view, members only";
    alwaysOn = true;
    base = {
      uses = {
        memory = "0";
        storage = "0";
      }; # nginx and the gate's
      network = {
        outbound = "none";
        inbound = "network";
      };
      depends = {
        thanos-query.local = true;
        nginx.local = true;
        dd-verify.local = true;
      };
      privacy = "metadata";
    };
  };

  disk-watch = {
    what = "smartd's tests of the box's own disks and the pools' health, as facts the fleet view sees";
    units = [
      "dd-disk-health"
      "smartd"
    ];
    everyBox = true;
    base = {
      # node1, 2026-10-04: smartd's peak
      uses = {
        memory = "35M";
        storage = "0";
      };
      network = {
        outbound = "none";
        inbound = "none";
      };
      depends.prometheus-node-exporter.local = true;
      privacy = "none";
    };
  };

  fleet-alerts = {
    what = "mails what is wrong on any box (disks, pools, backups), read from the fleet view";
    # a box that is down cannot say so: this has to stay up
    alwaysOn = true;
    units = [ "dd-fleet-alerts" ];
    instances = 1; # one mail, not one a box
    base = {
      uses = {
        memory = "16M";
        storage = "0";
        estimated = true; # a curl and a jq, every fifteen minutes
      };
      network = {
        outbound = [ "mail" ]; # mail.<domain>, the mail Worker
        inbound = "none";
      };
      depends.thanos-query.local = true;
      privacy = "metadata";
    };
  };
}
