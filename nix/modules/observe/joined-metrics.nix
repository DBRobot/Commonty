# A box a member set up (docs/dev/joining.md) has no key to the metrics
# bucket, which every release box's sidecar uploads to. It sends its
# numbers instead: its prometheus writes them to the box the plan gives
# joined-metrics, at metrics.<domain> over the store's port, the one a
# joined box reaches. There each joined box has a token of its own, sealed to it and
# served beside its store keys (/_keys/<box>-metrics.age); nginx names the
# box from the token and nothing else, so a box never writes as another.
# A Thanos receive keeps each joined box's series apart, labelled box=<its
# name>, and a query of its own answers for one box at a time: the Metrics
# page shows them to the box's member, whom the gates name, and to nobody
# else. The fleet's own view (thanos.nix) never reads them.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.joinedMetrics;
  base = config.dd.domain;
  state = "/var/lib/dd-metrics-joined";
  # the store's sealed keys, served at /_keys/ on its port (store.nix)
  sealed = "/var/lib/dd-store-sealed";
  map = "${state}/tokens.map";
  receiveTsdb = "/var/lib/thanos-receive";
  dropBox = pkgs.writeText "receive-relabel.yaml" ''
    # a box names itself by its token, never by a label it sends
    - action: labeldrop
      regex: "box|tenant_id"
  '';
in
{
  options.dd.joinedMetrics = {
    receive = lib.mkOption {
      type = lib.types.bool;
      default = (config.dd.thanos.sidecars or [ ]) != [ ] && (config.dd.store.enable or false);
      description = "take the metrics of the boxes that joined on their own keys: on the observe box";
    };
    to = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "a joined box: where it sends its metrics, https://metrics.<domain>:3443, the store's port of whichever box runs joined-metrics (the name follows it)";
    };
    maxBytes = lib.mkOption {
      type = lib.types.int;
      default = 1024 * 1024 * 1024;
      description = "what one joined box's series may take on the observe box's disk; past it, what it sends is refused until its oldest days are let go";
    };
    retention = lib.mkOption {
      type = lib.types.str;
      default = "15d";
    };
    queryPort = lib.mkOption {
      type = lib.types.port;
      default = 10913;
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.receive {
      systemd.services.thanos-receive-joined = {
        description = "Thanos receive for the joined boxes' metrics, each box apart";
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" ];
        serviceConfig = {
          DynamicUser = true;
          StateDirectory = "thanos-receive";
          Restart = "always";
          ExecStart = lib.concatStringsSep " " [
            "${pkgs.thanos}/bin/thanos receive"
            "--tsdb.path=${receiveTsdb}"
            "--tsdb.retention=${cfg.retention}"
            "--grpc-address=127.0.0.1:10911"
            "--http-address=127.0.0.1:10912"
            "--remote-write.address=127.0.0.1:10910"
            # receivers replicating to each other; there is one, and thanos
            # otherwise opens it on every address
            "--receive.capnproto-address=127.0.0.1:10915"
            # systemd keeps the quotes inside single ones; thanos wants them
            "'--label=receiver=\"joined\"'"
            "--receive.tenant-header=THANOS-TENANT"
            "--receive.tenant-label-name=box"
            # a write nginx did not name is nobody's, and nobody reads it
            "--receive.default-tenant-id=nobody"
            "--receive.relabel-config-file=${dropBox}"
          ];
        };
      };
      # one box's series at a time: the tenant nginx names, as a matcher on
      # every query, whatever the query says
      systemd.services.thanos-query-joined = {
        description = "Thanos query over the joined boxes' metrics, one box at a time";
        wantedBy = [ "multi-user.target" ];
        after = [ "thanos-receive-joined.service" ];
        serviceConfig = {
          DynamicUser = true;
          Restart = "always";
          ExecStart = lib.concatStringsSep " " [
            "${pkgs.thanos}/bin/thanos query"
            "--http-address=127.0.0.1:${toString cfg.queryPort}"
            "--grpc-address=127.0.0.1:${toString (cfg.queryPort + 1)}"
            "--endpoint=127.0.0.1:10911"
            "--query.enforce-tenancy"
            "--query.tenant-label-name=box"
            "--query.tenant-header=THANOS-TENANT"
            "--query.default-tenant-id=nobody"
          ];
        };
      };

      systemd.tmpfiles.rules = [
        "d ${state} 0750 root nginx -"
        "d ${state}/tokens 0700 root root -"
        "f ${map} 0640 root nginx -"
      ];
      # the map's keys are whole Authorization lines, longer than the
      # default buckets hold
      services.nginx.mapHashBucketSize = lib.mkDefault 128;
      services.nginx.appendHttpConfig = ''
        # a joined box, by its metrics token (dd-metrics-joined); none, nobody
        map $http_authorization $dd_metrics_box {
          default "";
          include ${map};
        }
        limit_req_zone $dd_metrics_box zone=dd_metrics_joined:1m rate=2r/s;
      '';
      # the joined boxes ask for metrics.<domain> on the store's port: the
      # name the DNS Worker points at whichever box runs this (needs hosts)
      services.nginx.virtualHosts."store.${config.networking.hostName}.${base}" = {
        serverAliases = [ "metrics.${base}" ];
        locations."= /_metrics/write" = {
          proxyPass = "http://127.0.0.1:10910/api/v1/receive";
          ddCookie = "";
          extraConfig = ''
            if ($dd_metrics_box = "") { return 403; }
            limit_except POST { deny all; }
            limit_req zone=dd_metrics_joined burst=10 nodelay;
            limit_req_status 429;
            client_max_body_size 1m;
            proxy_set_header Authorization "";
            proxy_set_header THANOS-TENANT $dd_metrics_box;
          '';
        };
      };
      # the Metrics page asks for one joined box: /joined/<its name>/api/v1/...,
      # once the gate has said it is this person's (lib.rs verify)
      services.nginx.virtualHosts."metrics.${base}".locations."~ ^/joined/(?<dd_jbox>j[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f])/api/v1/(?<dd_japi>query|query_range|series|labels|label/[a-zA-Z0-9_]+/values)$" =
        {
          proxyPass = "http://127.0.0.1:${toString cfg.queryPort}/api/v1/$dd_japi$is_args$args";
          ddCookie = "";
          extraConfig = ''
            limit_except GET POST { deny all; }
            auth_request /_dd/verify;
            error_page 401 = @login;
            error_page 403 = @waiting;
            proxy_set_header Authorization "";
            proxy_set_header THANOS-TENANT $dd_jbox;
          '';
        };

      # a token per joined box, sealed to it; gone with it from the agreed
      # state; and refused while its series take more than their share
      systemd.services.dd-metrics-joined = {
        description = "Metrics tokens for the boxes that joined on their own keys";
        after = [ "nginx.service" ];
        restartIfChanged = false;
        path = [
          pkgs.coreutils
          pkgs.diffutils # cmp: nginx reloads only when the tokens change
          pkgs.gnugrep
          pkgs.age
          pkgs.curl
          pkgs.jq
          pkgs.systemd
        ];
        serviceConfig.Type = "oneshot";
        script = ''
          set -uo pipefail
          st=$(curl -fsS --max-time 10 ${
            config.dd.store.control or "http://127.0.0.1:4195"
          }/state) || { echo "no agreed state yet"; exit 0; }
          umask 077
          mkdir -p ${sealed}
          now=$(printf '%s' "$st" | jq -r '.joined // {} | keys[]')
          out=$(mktemp)
          for p in $now; do
            case "$p" in j[0-9a-f]*) ;; *) continue ;; esac
            recipient=$(printf '%s' "$st" | jq -r --arg p "$p" '.joined[$p].recipient')
            # a token is for the box that joined with this recipient: one
            # with another under the same name gets a token of its own
            if [ "$(cat ${state}/tokens/$p.for 2>/dev/null)" != "$recipient" ]; then
              rm -f ${state}/tokens/$p ${sealed}/$p-metrics.age
              printf '%s' "$recipient" > ${state}/tokens/$p.for
            fi
            [ -s ${state}/tokens/$p ] || head -c 32 /dev/urandom | base64 | tr -d '/+=\n' > ${state}/tokens/$p
            if [ ! -s ${sealed}/$p-metrics.age ]; then
              if age -r "$recipient" -o ${sealed}/$p-metrics.age.new ${state}/tokens/$p; then
                chmod 0444 ${sealed}/$p-metrics.age.new
                mv ${sealed}/$p-metrics.age.new ${sealed}/$p-metrics.age
                echo "$p: its metrics token, sealed to it"
              else
                echo "$p: its recipient is not one age takes"
                continue
              fi
            fi
            used=$(du -sb ${receiveTsdb}/$p 2>/dev/null | cut -f1) || used=0
            if [ "''${used:-0}" -gt ${toString cfg.maxBytes} ]; then
              echo "$p: its series take $used bytes, over ${toString cfg.maxBytes}; refused for now"
              continue
            fi
            printf '"Bearer %s" %s;\n' "$(cat ${state}/tokens/$p)" "$p" >> $out
          done
          # a joined box gone from the agreed state: its token goes
          for f in ${state}/tokens/j*; do
            [ -e "$f" ] || continue
            p=$(basename "$f" .for)
            printf '%s\n' $now | grep -qx "$p" && continue
            rm -f ${state}/tokens/$p ${state}/tokens/$p.for ${sealed}/$p-metrics.age
            echo "$p is gone: its metrics token is too"
          done
          if ! cmp -s $out ${map}; then
            install -m 0640 -g nginx $out ${map}
            systemctl reload nginx || true
          fi
          rm -f $out
          exit 0
        '';
      };
      systemd.timers.dd-metrics-joined = {
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnActiveSec = "2min";
          OnUnitActiveSec = "5min";
        };
      };
    })

    (lib.mkIf (cfg.to != null) {
      # what it sends, labelled by nothing it chooses: the observe box names it
      services.prometheus.remoteWrite = [
        {
          url = "${cfg.to}/_metrics/write";
          bearer_token_file = "/var/lib/dd-metrics/token";
          queue_config = {
            max_samples_per_send = 2000;
            max_shards = 2;
          };
        }
      ];
      systemd.tmpfiles.rules = [
        "d /var/lib/dd-metrics 0750 root prometheus -"
        "f /var/lib/dd-metrics/token 0640 root prometheus -"
      ];
      # its token, sealed to this box at the observe box's store
      systemd.services.dd-metrics-key = {
        description = "This box's token for sending its metrics";
        restartIfChanged = false;
        path = [
          pkgs.coreutils
          pkgs.curl
          pkgs.age
        ]
        ++ config.dd.store.agePlugins;
        serviceConfig.Type = "oneshot";
        script = ''
          set -uo pipefail
          [ -s /var/lib/dd-metrics/token ] && exit 0
          t=$(mktemp)
          if curl -fsS --max-time 30 "${cfg.to}/_keys/${config.networking.hostName}-metrics.age" -o $t.age \
            && age -d -i ${config.dd.store.identity} $t.age > $t; then
            install -m 0640 -g prometheus $t /var/lib/dd-metrics/token
            echo "the metrics token, from the observe box"
          else
            echo "no metrics token yet"
          fi
          rm -f $t $t.age
          exit 0
        '';
      };
      systemd.timers.dd-metrics-key = {
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnActiveSec = "1min";
          OnUnitActiveSec = "5min";
        };
      };
    })
  ];
}
