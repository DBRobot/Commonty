# Whether the services members use work, measured by the box that runs
# them, every minute, into /var/lib/dd-facts/uptime.prom: the public uptime
# page reads it through thanos (servers/gate/src/status.rs). A box looks at
# its own units and its own addresses, never another box's. A service on
# two boxes is measured on each, and the page takes the worse.
{
  ddScript,
  config,
  pkgs,
  lib,
  ...
}:
let
  has = name: config.systemd.services ? ${name};
  local = lib.filter has;
  gate = config.dd.verify.enable && config.dd.verify.role == "full";
  web = gate && has "nginx";
  front = local [
    "dd-verify"
    "nginx"
  ];
  tunnels = lib.filter (lib.hasPrefix "cloudflared-tunnel-") (
    builtins.attrNames config.systemd.services
  );
  library = web && config.dd.verify.library != null;
  # id, the units it needs on this box, an address that should answer
  checks =
    lib.optional web {
      id = "site";
      units = front ++ tunnels;
      url = "http://127.0.0.1:${toString config.dd.verify.port}/health";
    }
    ++ lib.optional library {
      id = "files";
      units = front ++ local [ "garage" ];
    }
    ++ lib.optional (library || has "dd-transcode") {
      id = "movies";
      units = lib.optionals library (front ++ local [ "garage" ]) ++ local [ "dd-transcode" ];
    }
    ++ lib.optional (has "ente") {
      id = "photos";
      units = [ "ente" ] ++ local [
        "postgresql"
        "nginx"
      ];
      url = "http://127.0.0.1:8080/ping";
    }
    ++ lib.optional (has "vaultwarden") {
      id = "passwords";
      units = [ "vaultwarden" ] ++ local [
        "postgresql"
        "nginx"
      ];
      url = "http://127.0.0.1:${toString config.services.vaultwarden.config.ROCKET_PORT}/alive";
    }
    ++ lib.optional (has "llama-swap") {
      id = "chat";
      units = [ "llama-swap" ] ++ local [
        "llm-proxy"
        "nginx"
      ];
    }
    ++ lib.optional (has "dd-games") {
      id = "games";
      units = [ "dd-games" ];
    }
    ++ lib.optional (has "forgejo") {
      id = "git";
      units = [ "forgejo" ] ++ local [
        "postgresql"
        "nginx"
      ];
    };
  list = pkgs.writeText "uptime-checks" (
    lib.concatMapStrings (c: "${c.id} ${lib.concatStringsSep "," c.units} ${c.url or "-"}\n") checks
  );
in
{
  systemd.services.dd-uptime = {
    description = "Note whether the services members use work here";
    restartIfChanged = false;
    path = [
      pkgs.systemd
      pkgs.curl
      pkgs.coreutils
    ];
    serviceConfig = {
      Type = "oneshot";
      User = "node-exporter";
      Group = "node-exporter";
    };
    script = ddScript ./uptime.sh {
      FACTS = "/var/lib/dd-facts";
      LIST = list;
    };
  };
  systemd.timers.dd-uptime = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnActiveSec = "1min";
      OnUnitActiveSec = "1min";
    };
  };
}
