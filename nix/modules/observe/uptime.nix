# Whether the services members use work, measured by the box that runs
# them, every minute, into /var/lib/dd-facts/uptime.prom: the public uptime
# page reads it through thanos (servers/gate/src/status.rs). A box looks at
# its own units and its own addresses, never another box's. A placed
# service (needs placed or lease) is noted only by the box the plan runs it
# on; one on several boxes is noted on each, and the page takes the worse.
# Only the boxes the release builds with a service may speak for it
# (dd.verify.serviceBoxes, from nix/lib/fleet-placement.nix builtWith).
{
  ddScript,
  config,
  pkgs,
  lib,
  ...
}:
let
  needs = import ../../lib/needs.nix {
    inherit lib;
    on = config.dd.failover or false;
  };
  fp = import ../../lib/fleet-placement.nix { inherit lib; };
  # each id the page shows, by the needs' services that bring it
  brings = {
    site = [ "cloudflared" ];
    files = [ "disk-images" ];
    movies = [
      "disk-images"
      "dd-transcode"
    ];
    photos = [ "ente-museum" ];
    passwords = [ "vaultwarden" ];
    chat = [ "llama-swap" ];
    games = [ "dd-games" ];
    git = [ "forgejo" ];
  };
  has = name: config.systemd.services ? ${name};
  local = lib.filter has;
  # the flag the plan puts where a placed service runs (box/placed.nix);
  # "-" for one that is not switched
  switched =
    s: (config.dd.control.enable or false) && needs ? ${s} && (needs.${s}.placed || needs.${s}.lease);
  flag = s: if switched s then s else "-";
  gate = config.dd.verify.enable && config.dd.verify.role == "full";
  web = gate && has "nginx";
  front = local [
    "dd-verify"
    "nginx"
  ];
  # the public door: the one tunnel's unit, or the box's own (gate/public.nix)
  tunnels =
    lib.filter (lib.hasPrefix "cloudflared-tunnel-") (builtins.attrNames config.systemd.services)
    ++ local [ "dd-door" ];
  library = web && config.dd.verify.library != null;
  # id, the units it needs on this box, an address that should answer, the
  # placed service whose flag says it runs here
  checks =
    lib.optional (web && tunnels != [ ]) {
      id = "site";
      units = front ++ tunnels;
      url = "http://127.0.0.1:${toString config.dd.verify.port}/health";
      placed = flag "cloudflared";
    }
    ++ lib.optional library {
      id = "files";
      units = front ++ local [ "garage" ];
    }
    ++ lib.optional (library || has "dd-transcode") {
      id = "movies";
      units = lib.optionals library (front ++ local [ "garage" ]) ++ local [ "dd-transcode" ];
      placed = if has "dd-transcode" then flag "dd-transcode" else "-";
    }
    ++ lib.optional (has "ente") {
      id = "photos";
      units = [
        "ente"
      ]
      ++ local [
        "postgresql"
        "nginx"
      ];
      url = "http://127.0.0.1:8080/ping";
      placed = flag "ente-museum";
    }
    ++ lib.optional (has "vaultwarden") {
      id = "passwords";
      units = [
        "vaultwarden"
      ]
      ++ local [
        "postgresql"
        "nginx"
      ];
      url = "http://127.0.0.1:${toString config.services.vaultwarden.config.ROCKET_PORT}/alive";
      placed = flag "vaultwarden";
    }
    ++ lib.optional (has "llama-swap") {
      id = "chat";
      units = [
        "llama-swap"
      ]
      ++ local [
        "llm-proxy"
        "nginx"
      ];
      placed = flag "llama-swap";
    }
    ++ lib.optional (has "dd-games") {
      id = "games";
      units = [ "dd-games" ];
      placed = flag "dd-games";
    }
    ++ lib.optional (has "forgejo") {
      id = "git";
      units = [
        "forgejo"
      ]
      ++ local [
        "postgresql"
        "nginx"
      ];
      placed = flag "forgejo";
    };
  list = pkgs.writeText "uptime-checks" (
    lib.concatMapStrings (
      c: "${c.id} ${lib.concatStringsSep "," c.units} ${c.url or "-"} ${c.placed or "-"}\n"
    ) checks
  );
in
{
  dd.verify.serviceBoxes = lib.mkDefault (
    lib.mapAttrs (_: ss: lib.unique (lib.concatMap fp.builtWith ss)) brings
  );
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
