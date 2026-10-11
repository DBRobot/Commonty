# This box's own store: a garage of one, with a secret it made itself and
# shares with nobody (docs/dev/storage.md). Services here write to it on
# 127.0.0.1; what they write is ciphertext, and a copy goes to other boxes'
# stores, so it is kept in a set number of places and any of them can serve
# it when a service moves.
#
# - A key per bucket for the service on this box that uses it, made here,
#   kept in a file only that service reads.
# - A key per other box, for the copies it sends here: written into the
#   buckets it copies, nothing else. Handed to that box sealed to its own
#   secrets key (its TPM), at https://store.<this box>.<domain>/_keys/<it>.age,
#   which only it can open; the name is on the fleet's network alone.
# - Copies go out every 5 minutes to the first boxes by name, as many as
#   the bucket's copies ask. Nothing is deleted on the other end: a copy is
#   added to, never pruned.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.store;
  box = config.networking.hostName;
  base = config.dd.domain;
  state = "/var/lib/dd-store";
  # the keys handed to other boxes, each sealed to its box: public, so
  # nginx serves them, and outside the store's own state
  sealed = "/var/lib/dd-store-sealed";
  conf = (pkgs.formats.toml { }).generate "dd-store.toml" {
    metadata_dir = "${state}/meta";
    data_dir = cfg.dataDir;
    db_engine = "lmdb";
    replication_factor = 1;
    consistency_mode = "consistent";
    metadata_auto_snapshot_interval = "1d";
    rpc_secret_file = "${state}/rpc-secret";
    # a store of one: its rpc is for its own cli, never another box
    rpc_bind_addr = "127.0.0.1:${toString cfg.rpcPort}";
    rpc_public_addr = "127.0.0.1:${toString cfg.rpcPort}";
    s3_api = {
      s3_region = "us-east-1";
      api_bind_addr = "127.0.0.1:${toString cfg.port}";
    };
    admin.api_bind_addr = "127.0.0.1:${toString cfg.adminPort}";
  };
  garage = "${pkgs.garage}/bin/garage -c ${conf}";
  # the other boxes copies go to, by name: the first n-1 of them
  sorted = lib.sort lib.lessThan (builtins.attrNames cfg.peers);
  targets = n: lib.take (n - 1) sorted;
  bucketsTo =
    p: lib.attrNames (lib.filterAttrs (_: b: builtins.elem p (targets b.copies)) cfg.buckets);
  # which of the other boxes' buckets come here: every bucket of ours they
  # might copy, since each names its own targets
  received = lib.attrNames (lib.filterAttrs (_: b: b.copies > 1) cfg.buckets);
in
{
  options.dd.store = {
    enable = lib.mkEnableOption "this box's own store, its copies on other boxes' stores";
    dataDir = lib.mkOption {
      type = lib.types.str;
      default = "/srv/store";
    };
    capacity = lib.mkOption {
      type = lib.types.str;
      description = "what this store offers, e.g. \"3T\"";
    };
    port = lib.mkOption {
      type = lib.types.port;
      default = 3910;
    };
    rpcPort = lib.mkOption {
      type = lib.types.port;
      default = 3911;
    };
    adminPort = lib.mkOption {
      type = lib.types.port;
      default = 3913;
    };
    identity = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/dd/secrets-key.txt";
      description = "this box's secrets key: what opens the keys other boxes hand it";
    };
    agePlugins = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ pkgs.age-plugin-tpm ];
    };
    peers = lib.mkOption {
      type = lib.types.attrsOf (
        lib.types.submodule {
          options = {
            url = lib.mkOption {
              type = lib.types.str;
              description = "its store, as this box reaches it";
            };
            recipient = lib.mkOption {
              type = lib.types.str;
              description = "its secrets key's recipient: what its key here is sealed to";
            };
          };
        }
      );
      default = { };
      description = "the other boxes with a store";
    };
    buckets = lib.mkOption {
      type = lib.types.attrsOf (
        lib.types.submodule {
          options = {
            copies = lib.mkOption {
              type = lib.types.ints.positive;
              default = 2;
              description = "in how many stores it is kept, this one included";
            };
            owner = lib.mkOption {
              type = lib.types.str;
              default = "root";
              description = "who reads the bucket's key file, ${state}/keys/<bucket>.env";
            };
          };
        }
      );
      default = { };
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.dd-store = {
      isSystemUser = true;
      group = "dd-store";
      home = state;
    };
    users.groups.dd-store = { };
    systemd.tmpfiles.rules = [
      "d ${state} 0750 dd-store dd-store -"
      "d ${state}/meta 0700 dd-store dd-store -"
      "d ${state}/keys 0755 root root -"
      "d ${sealed} 0755 root root -"
      "d ${state}/from 0700 root root -"
      "d ${cfg.dataDir} 0750 dd-store dd-store -"
    ];
    environment.systemPackages = [
      (pkgs.writeShellScriptBin "dd-store-garage" ''exec ${garage} "$@"'')
    ];

    systemd.services.dd-store = {
      description = "This box's own store";
      wantedBy = [ "multi-user.target" ];
      after = [ "zfs-datasets.service" ];
      wants = [ "zfs-datasets.service" ];
      unitConfig.RequiresMountsFor = [ cfg.dataDir ];
      serviceConfig = {
        User = "dd-store";
        Group = "dd-store";
        # its secret, made here once; no other box ever has it
        ExecStartPre = [
          "+${pkgs.writeShellScript "dd-store-secret" ''
            umask 077
            [ -s ${state}/rpc-secret ] || ${pkgs.openssl}/bin/openssl rand -hex 32 > ${state}/rpc-secret
            chown dd-store:dd-store ${state}/rpc-secret
            ${pkgs.coreutils}/bin/install -d -o dd-store -g dd-store -m 0750 ${cfg.dataDir}
          ''}"
        ];
        ExecStart = "${pkgs.garage}/bin/garage -c ${conf} server";
        Restart = "always";
        RestartSec = 5;
        StateDirectory = "dd-store";
        StateDirectoryMode = "0750";
        ReadWritePaths = [ cfg.dataDir ];
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        NoNewPrivileges = true;
      };
    };

    # its layout, its buckets and the keys: converging, safe to run again
    systemd.services.dd-store-setup = {
      description = "This box's store: its layout, buckets and keys";
      after = [ "dd-store.service" ];
      requires = [ "dd-store.service" ];
      wantedBy = [ "multi-user.target" ];
      restartIfChanged = true;
      path = [
        pkgs.coreutils
        pkgs.gnugrep
        pkgs.gawk
        pkgs.age
      ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        set -euo pipefail
        g() { ${garage} "$@"; }
        for _ in $(seq 1 60); do g status >/dev/null 2>&1 && break; sleep 1; done
        node=$(g node id -q | cut -d@ -f1)
        if ! g layout show 2>/dev/null | grep -q "Current cluster layout version: [1-9]"; then
          g layout assign -z ${box} -c ${cfg.capacity} "$node"
          g layout apply --version 1
        fi
        # a key's id and secret, made once, read back with its secret
        key() {
          g key info "$1" >/dev/null 2>&1 || g key create "$1" >/dev/null
          g key info "$1" --show-secret | awk '
            /Key ID:/ { print "AWS_ACCESS_KEY_ID=" $NF }
            /Secret key:/ { print "AWS_SECRET_ACCESS_KEY=" $NF }'
        }
        umask 077
        ${lib.concatStrings (
          lib.mapAttrsToList (b: v: ''
            g bucket create ${b} 2>/dev/null || true
            key local-${b} > ${state}/keys/${b}.env.new
            id=$(awk -F= '/ACCESS_KEY_ID/ { print $2 }' ${state}/keys/${b}.env.new)
            g bucket allow --read --write ${b} --key "$id" >/dev/null
            chown ${v.owner} ${state}/keys/${b}.env.new
            chmod 0400 ${state}/keys/${b}.env.new
            mv ${state}/keys/${b}.env.new ${state}/keys/${b}.env
          '') cfg.buckets
        )}
        # each other box's key here: written into what it copies, sealed to it
        ${lib.concatStrings (
          lib.mapAttrsToList (p: v: ''
            key peer-${p} > ${state}/from/${p}.env
            id=$(awk -F= '/ACCESS_KEY_ID/ { print $2 }' ${state}/from/${p}.env)
            for b in ${lib.concatStringsSep " " received}; do
              g bucket allow --read --write "$b" --key "$id" >/dev/null
            done
            age -r ${lib.escapeShellArg v.recipient} -o ${sealed}/${p}.age.new ${state}/from/${p}.env
            chmod 0444 ${sealed}/${p}.age.new
            mv ${sealed}/${p}.age.new ${sealed}/${p}.age
          '') cfg.peers
        )}
      '';
    };

    # the copies: this box's buckets, out to the stores they are kept in
    systemd.services.dd-store-copy = {
      description = "Copy this box's store out to the stores its buckets are kept in";
      after = [ "dd-store-setup.service" ];
      restartIfChanged = false;
      path = [
        pkgs.rclone
        pkgs.curl
        pkgs.age
        pkgs.coreutils
        pkgs.gawk
      ]
      ++ cfg.agePlugins;
      serviceConfig = {
        Type = "oneshot";
        StateDirectory = "dd-store-copy";
      };
      script = ''
        set -uo pipefail
        umask 077
        # everything rclone needs is in its environment below
        export RCLONE_CONFIG=/dev/null
        mine=/var/lib/dd-store-copy
        failed=0
        ${lib.concatStrings (
          map (
            p:
            let
              url = cfg.peers.${p}.url;
            in
            lib.optionalString (bucketsTo p != [ ]) ''
              # its key here, sealed to this box at its store
              if [ ! -s $mine/${p}.env ]; then
                if curl -fsS --max-time 30 ${url}/_keys/${box}.age -o $mine/${p}.age \
                  && age -d -i ${cfg.identity} $mine/${p}.age > $mine/${p}.env.new; then
                  mv $mine/${p}.env.new $mine/${p}.env
                else
                  echo "${p}: no key from its store yet"
                  failed=1
                fi
              fi
              if [ -s $mine/${p}.env ]; then
                set -a; . $mine/${p}.env; set +a
                export RCLONE_CONFIG_THERE_TYPE=s3 RCLONE_CONFIG_THERE_PROVIDER=Other \
                  RCLONE_CONFIG_THERE_ENDPOINT=${url} RCLONE_CONFIG_THERE_REGION=us-east-1 \
                  RCLONE_CONFIG_THERE_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID \
                  RCLONE_CONFIG_THERE_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY \
                  RCLONE_CONFIG_THERE_FORCE_PATH_STYLE=true
                for b in ${lib.concatStringsSep " " (bucketsTo p)}; do
                  set -a; . ${state}/keys/$b.env; set +a
                  export RCLONE_CONFIG_HERE_TYPE=s3 RCLONE_CONFIG_HERE_PROVIDER=Other \
                    RCLONE_CONFIG_HERE_ENDPOINT=http://127.0.0.1:${toString cfg.port} RCLONE_CONFIG_HERE_REGION=us-east-1 \
                    RCLONE_CONFIG_HERE_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID \
                    RCLONE_CONFIG_HERE_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY \
                    RCLONE_CONFIG_HERE_FORCE_PATH_STYLE=true
                  # copy, never sync: nothing is deleted there
                  if rclone copy --checksum --s3-no-check-bucket "here:$b" "there:$b"; then
                    date +%s > $mine/${p}.$b.ok
                  else
                    echo "${p}: $b not copied this time"
                    failed=1
                  fi
                done
              fi
            ''
          ) sorted
        )}
        # a box away is not this box failing: the timer comes back
        [ "$failed" = 0 ] || echo "some copies wait for the next run"
        exit 0
      '';
    };
    systemd.timers.dd-store-copy = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "2min";
        OnUnitActiveSec = "5min";
      };
    };

    # other boxes reach this store, and their keys to it, by name on the
    # fleet's network
    services.nginx.virtualHosts."store.${box}.${base}" = {
      useACMEHost = base;
      forceSSL = true;
      locations."/".proxyPass = "http://127.0.0.1:${toString cfg.port}";
      # s3 signs its requests; no cookie of anyone's goes to the store
      locations."/".ddCookie = "";
      locations."/".extraConfig = ''
        client_max_body_size 0;
        proxy_request_buffering off;
        proxy_set_header Host $host;
      '';
      locations."/_keys/".alias = "${sealed}/";
    };
  };
}
