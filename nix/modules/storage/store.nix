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
# - Copies go out every 5 minutes from the box that writes the bucket (where
#   its service runs) to the boxes the plan keeps it on: the control plane
#   picks them by room and keeps a copy where it is (place.Decide's Copies,
#   which dd-control writes to /run/dd-placed/copies.json). Before any plan
#   has been seen, the first boxes by name. Nothing is deleted on the other
#   end: a copy is added to, never pruned. A writer handed over is copied
#   once more, stopped and before its flag goes (dd-store-copy-final@).
# - A store says how big it is, what it has free and what it keeps of each
#   bucket (/var/lib/dd-facts/store.prom, in the box's heartbeat), and
#   refuses what other boxes send once it is nearly full. Its dataset's
#   quota is its capacity, so its own services cannot fill the pool either.
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
  needs = import ../../lib/needs.nix {
    inherit lib;
    on = config.dd.failover or false;
  };
  # every bucket a service writes, fleet-wide: any store may be picked to
  # keep any of them. One a box writes alone (perBox) is one bucket for
  # each box, <name>-<box>, which only that box copies out
  declared = lib.foldl' (a: b: a // b) { } (
    lib.concatLists (
      lib.mapAttrsToList (
        s: n:
        lib.mapAttrsToList (
          name: b:
          if b.perBox then
            lib.listToAttrs (
              map (
                x:
                lib.nameValuePair "${name}-${x}" {
                  inherit (b) copies shared;
                  service = s;
                  writer = x;
                }
              ) cfg.boxes
            )
          else
            {
              ${name} = {
                inherit (b) copies shared;
                service = s;
              };
            }
        ) n.buckets
      ) needs
    )
  );
  sorted = lib.sort lib.lessThan (builtins.attrNames cfg.peers);
  # what is copied anywhere, and so may come here from other boxes
  copied = lib.attrNames (lib.filterAttrs (_: b: b.copies > 1) cfg.buckets);
  # what other boxes write here, each with its key, when this is its home
  sharedHere = lib.attrNames (lib.filterAttrs (_: b: b.shared && b.home == box) cfg.buckets);
  # every bucket a peer's key here is allowed: its copies, and what it writes here
  peerBuckets = lib.unique (copied ++ sharedHere);
  homed = lib.filterAttrs (_: b: b.shared && b.home != null) cfg.buckets;
  # the plan, as dd-control writes it, and the last one this box saw
  plan = "/run/dd-placed/copies.json";
  # a placed service's bucket is written where the service runs; another's,
  # wherever this box has it
  placedSvc = s: needs ? ${s} && (needs.${s}.placed || needs.${s}.lease);
  full = "/run/dd-store/full";
  corsRule = {
    CORSRules = [
      {
        AllowedOrigins = [ "*" ];
        AllowedMethods = [
          "GET"
          "PUT"
          "POST"
          "DELETE"
          "HEAD"
        ];
        AllowedHeaders = [ "*" ];
        ExposeHeaders = [
          "etag"
          "ETag"
          "x-amz-request-id"
        ];
        MaxAgeSeconds = 3000;
      }
    ];
  };
  # the copies: this box's buckets, out to the stores they are kept in.
  # Every 30 seconds what changed in the last quarter hour, so a takeover
  # finds all but the last moments there; every half hour all of it, with
  # a copy read back. And once more when a writer is handed over (final@<its
  # service>, from dd-control), stopped and its flag still up: what changed
  # in its own buckets since, done before the lease may lapse
  copyUnit = mode: {
    description =
      {
        all = "Copy this box's store out to the stores its buckets are kept in";
        recent = "Copy what changed in this box's store out to the stores its buckets are kept in";
        final = "Copy a handed-over service's last writes out to the stores its buckets are kept in";
      }
      .${mode};
    environment.MODE = mode;
    scriptArgs = lib.mkIf (mode == "final") "%i";
    after = [ "dd-store-setup.service" ];
    restartIfChanged = false;
    path = [
      pkgs.rclone
      pkgs.curl
      pkgs.age
      pkgs.coreutils
      pkgs.gawk
      pkgs.gnugrep
      pkgs.jq
      pkgs.util-linux
    ]
    ++ cfg.agePlugins;
    serviceConfig = {
      Type = "oneshot";
      StateDirectory = "dd-store-copy";
      # dd-control stops it sooner, at the lease's until
      TimeoutStartSec = lib.mkIf (mode == "final") "1min";
    };
    script = ''
      set -uo pipefail
      umask 077
      # everything rclone needs is in its environment below
      export RCLONE_CONFIG=/dev/null
      mine=/var/lib/dd-store-copy
      mkdir -p $mine/lost /run/dd-store
      failed=0
      # one run of each kind at a time; the two side by side is fine
      if [ "$MODE" != final ]; then
        exec 9>/run/dd-store/copy-$MODE.lock
        flock -n 9 || { echo "a $MODE copy still runs"; exit 0; }
        bounded() { "$@"; }
      else
        SVC=$1
        lapses=$(jq -r '.until // empty' "/run/dd-placed/$SVC" 2>/dev/null) || lapses=""
        [ -n "$lapses" ] || { echo "$SVC: its lease is not held here: nothing to copy"; exit 0; }
        read -r up _ </proc/uptime
        now=$((''${up%.*} * 1000 + 10#''${up#*.} * 10))
        # done a margin before the flag's until, by the same clock
        deadline=$(($(date +%s) + (lapses - now) / 1000 - 3))
        left() { echo $((deadline - $(date +%s))); }
        bounded() { [ "$(left)" -gt 0 ] && timeout "$(left)" "$@"; }
        # a final copy that fails says so: dd-control keeps the flag and asks again
        [ "$(left)" -gt 0 ] || { echo "$SVC: no time left before its lease may lapse"; exit 1; }
        # its own copies into this store first (the forge's)
        case "$SVC" in
        ${lib.concatStrings (
          lib.mapAttrsToList (s: us: "  ${s}) first=\"${lib.concatStringsSep " " us}\" ;;\n") cfg.handover
        )}
        *) first="" ;;
        esac
        for u in $first; do
          bounded systemctl start "$u" || { systemctl stop "$u"; echo "$SVC: $u failed or not done in time"; failed=1; }
        done
        # after a recent copy still running, never beside it
        exec 9>/run/dd-store/copy-recent.lock
        [ "$(left)" -gt 0 ] && flock -w "$(left)" 9 || { echo "$SVC: no final copy, a recent one ran past its time"; exit 1; }
      fi
      # An age filter reads each object's age, and rclone's own is a header
      # on the object: a HEAD for every object on both sides, every run.
      # node2's every 30 s was 15000 requests at node1 to copy nothing
      # (2026-10-24). The store's own time, from the listing, asks none.
      if [ "$MODE" = all ]; then recent=""; else recent="--max-age 15m --use-server-modtime"; fi
      # the plan's copies: kept, so a restart copies on as last agreed
      if [ -s ${plan} ] && jq -e 'type == "object"' ${plan} >/dev/null 2>&1; then
        # a name of this mode's own: the other mode's run, side by side, moved a
        # shared one away first and this one's mv failed the unit
        cp ${plan} $mine/copies.json.$MODE && mv $mine/copies.json.$MODE $mine/copies.json
      fi
      # the other boxes that keep a bucket
      targets() {
        if [ -s $mine/copies.json ]; then
          jq -r --arg b "$1" '.[$b] // [] | .[]' $mine/copies.json | grep -vx ${box} || true
        else
          # no plan seen yet: the first boxes by name
          case "$1" in
          ${lib.concatStrings (
            lib.mapAttrsToList (
              b: v: "  ${b}) echo ${lib.concatStringsSep " " (lib.take (v.copies - 1) sorted)} ;;\n"
            ) cfg.buckets
          )}
          esac
        fi
      }
      # a final copy's: the buckets its service writes
      owns() {
        [ "$MODE" = final ] || return 0
        case "$1" in
        ${lib.concatStrings (
          lib.mapAttrsToList (
            b: v: "  ${b}) [ \"$SVC\" = ${lib.escapeShellArg (toString v.service)} ] ;;\n"
          ) (lib.filterAttrs (_: v: v.service != null) cfg.buckets)
        )}
        *) false ;;
        esac
      }
      # whether this box writes it now
      writes() {
        case "$1" in
        ${lib.concatStrings (
          lib.mapAttrsToList (
            b: v:
            # a box's own: written there alone
            if v.writer != null then
              "  ${b}) ${if v.writer == box then "true" else "false"} ;;\n"
            else if v.service != null && placedSvc v.service then
              "  ${b}) [ -e /run/dd-placed/${v.service} ] ;;\n"
            else
              "  ${b}) true ;;\n"
          ) cfg.buckets
        )}
        *) false ;;
        esac
      }
      url() {
        case "$1" in
        ${lib.concatStrings (lib.mapAttrsToList (p: v: "  ${p}) echo ${v.url} ;;\n") cfg.peers)}
        j[0-9a-f]*)
          # a box that joined on its own key (state/join.go), as the agreed
          # state has it: its store is at its name
          curl -fsS --max-time 10 ${cfg.control}/state \
            | jq -e --arg n "$1" '.joined[$n] // empty' >/dev/null \
            && echo "https://store.$1.${base}:${toString cfg.publicPort}"
          ;;
        *) return 1 ;;
        esac
      }
      # its key there, sealed to this box at its store
      key() {
        [ -s $mine/$1.env ] && return 0
        if curl -fsS --max-time 30 "$2/_keys/${box}.age" -o $mine/$1.age.$MODE \
          && age -d -i ${cfg.identity} $mine/$1.age.$MODE > $mine/$1.env.$MODE; then
          mv $mine/$1.env.$MODE $mine/$1.env
        else
          echo "$1: no key from its store yet"
          return 1
        fi
      }
      for b in ${lib.concatStringsSep " " copied}; do
        owns $b && writes $b || continue
        for p in $(targets $b); do
          there=$(url $p) || { echo "$p: keeps $b by the plan, and is no store this box knows"; failed=1; continue; }
          key $p "$there" || { failed=1; continue; }
          (
            set -a; . $mine/$p.env; set +a
            export RCLONE_CONFIG_THERE_TYPE=s3 RCLONE_CONFIG_THERE_PROVIDER=Other \
              RCLONE_CONFIG_THERE_ENDPOINT=$there RCLONE_CONFIG_THERE_REGION=us-east-1 \
              RCLONE_CONFIG_THERE_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID \
              RCLONE_CONFIG_THERE_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY \
              RCLONE_CONFIG_THERE_FORCE_PATH_STYLE=true
            set -a; . ${state}/keys/$b.env; set +a
            export RCLONE_CONFIG_HERE_TYPE=s3 RCLONE_CONFIG_HERE_PROVIDER=Other \
              RCLONE_CONFIG_HERE_ENDPOINT=http://127.0.0.1:${toString cfg.port} RCLONE_CONFIG_HERE_REGION=us-east-1 \
              RCLONE_CONFIG_HERE_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID \
              RCLONE_CONFIG_HERE_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY \
              RCLONE_CONFIG_HERE_FORCE_PATH_STYLE=true
            # first, one object the last copy there sent, read back: a
            # store that takes copies and keeps none is found out
            if [ "$MODE" = all ] && [ -s $mine/$p.$b.ok ]; then
              age=$(( $(date +%s) - $(cat $mine/$p.$b.ok) ))
              obj=$(rclone lsf -R --files-only --fast-list --use-server-modtime --min-age "''${age}s" "here:$b" 2>/dev/null | shuf -n1 || true)
              if [ -n "$obj" ] && want=$(rclone cat "here:$b/$obj" | sha256sum); then
                if got=$(rclone cat "there:$b/$obj" 2>/dev/null | sha256sum) && [ "$got" = "$want" ]; then
                  rm -f $mine/lost/$p.$b
                else
                  echo "$p: $b/$obj read back is not what was sent: its copy there counts for nothing"
                  touch $mine/lost/$p.$b
                fi
              fi
            fi
            # copy, never sync: nothing is deleted there
            bounded rclone copy --checksum --s3-no-check-bucket --fast-list $recent "here:$b" "there:$b"
          ) && { [ "$MODE" != all ] || date +%s > $mine/$p.$b.ok; } || { echo "$p: $b not copied this time"; failed=1; }
        done
      done
      if [ "$MODE" = final ]; then
        [ "$failed" = 0 ] || { echo "$SVC: its final copy is not all there"; exit 1; }
        echo "$SVC: its last writes copied out"
      fi
      # what the plan reads of the copies read back (place.Decide)
      [ "$MODE" = all ] || exit 0
      out=$(mktemp)
      for f in $mine/lost/*; do
        [ -e "$f" ] || continue
        n=''${f##*/}
        echo "dd_store_lost{bucket=\"''${n#*.}\",box=\"''${n%%.*}\"} 1" >> $out
      done
      install -d /var/lib/dd-facts
      chmod 0644 $out
      mv $out /var/lib/dd-facts/store-copy.prom
      # a box away or full is not this box failing: the timer comes back
      [ "$failed" = 0 ] || echo "some copies wait for the next run"
      exit 0
    '';
  };
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
      default = "auto";
      description = "what this store offers, e.g. \"3T\"; auto: what it keeps and what its pool has free, less a fifth of the pool, kept for the box's other services and measured again every minute";
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
            cors = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "browsers send to it straight, with a presigned url (ente's photos): any origin, as the signature is the access control and a web worker's origin is null";
            };
            service = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              description = "the service that writes it (its needs' buckets); null: this box writes it whenever it has it";
            };
            writer = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              description = "the one box that writes it, and the only one that copies it out (a box's backup, the libraries); null: as its service says";
            };
            shared = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "kept in one box's store (home), and every box writes it there with its own key to that store";
            };
            home = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              description = "a shared bucket's box: whose store keeps it";
            };
          };
        }
      );
      default = { };
    };
    boxes = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ box ];
      description = "every box with a store: a bucket each box writes alone (needs perBox) is one bucket for each";
    };
    at = lib.mkOption {
      type = lib.types.attrsOf lib.types.attrs;
      readOnly = true;
      default = lib.mapAttrs (b: v: rec {
        endpoint =
          if v.home == box then "http://127.0.0.1:${toString cfg.port}" else cfg.peers.${v.home}.url;
        # the bucket as nix names an s3 store (the nix cache). The endpoint
        # whole, scheme and all: nix reads a bare "store.<box>.<domain>:3443"
        # as a url whose scheme is the host, drops &scheme=, and curl sends
        # it plain http to the TLS port - nginx's 400 to every narinfo from
        # node2 (2026-10-25)
        s3Url = "s3://${b}?endpoint=${endpoint}&region=us-east-1";
        # the key there: this box's own for the bucket at home, its key to
        # the home's store elsewhere (dd-store-at)
        envFile = "${state}/at/${b}.env";
      }) homed;
      description = "where this box reaches each shared bucket, and the file with its key there (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY), readable by the bucket's owner";
    };
    handover = lib.mkOption {
      type = lib.types.attrsOf (lib.types.listOf lib.types.str);
      default = { };
      description = "a service's units run to their end, in order, on its planned handover, before its final copy (the forge's own copy into this store)";
    };
    publicPort = lib.mkOption {
      type = lib.types.port;
      default = 3443;
      description = "where other boxes reach this store, at store.<box>.<domain>, and the only port a box that joined on its own key reaches";
    };
    control = lib.mkOption {
      type = lib.types.str;
      default = "http://127.0.0.1:4195";
      description = "this box's dd-control, whose agreed state lists the boxes that joined on their own keys";
    };
    dataset = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "the ZFS dataset dataDir is; its quota is set to the capacity";
    };
  };

  config = lib.mkIf cfg.enable {
    dd.store.buckets = lib.mapAttrs (
      _: b:
      {
        copies = lib.mkDefault b.copies;
        service = lib.mkDefault b.service;
        shared = lib.mkDefault b.shared;
      }
      // lib.optionalAttrs (b ? writer) { writer = lib.mkDefault b.writer; }
    ) declared;

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
      "d /run/dd-store 0755 root root -"
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
            # its metadata, as a reinstall carries it in (scripts/install-box), root's
            chown -R dd-store:dd-store ${state}/meta
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
      ]
      # it seals each other box's key to that box's secrets key, which is a
      # TPM's: without the plugin age cannot, and release 277 rolled back
      ++ cfg.agePlugins
      ++ lib.optional (cfg.dataset != null) config.boot.zfs.package;
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        set -euo pipefail
        g() { ${garage} "$@"; }
        for _ in $(seq 1 60); do g status >/dev/null 2>&1 && break; sleep 1; done
        node=$(g node id -q | cut -d@ -f1)
        ${lib.optionalString (cfg.dataset != null && cfg.capacity != "auto") ''
          # its own services cannot fill the pool past what it offers
          q=$(numfmt --from=iec ${cfg.capacity})
          [ "$(zfs get -Hp -o value quota ${cfg.dataset})" = "$q" ] || zfs set quota=$q ${cfg.dataset}
        ''}
        # read whole, then matched: grep -q leaving early fails the pipe
        layout=$(g layout show 2>/dev/null || true)
        if ! grep -q "Current cluster layout version: [1-9]" <<<"$layout"; then
          # a store of one: what it is told it offers only weighs it
          # against itself, so with auto, what the pool has now
          ${
            if cfg.capacity == "auto" then
              "cap=$(( $(df -B1 --output=avail ${cfg.dataDir} | tail -1) / 1048576 ))M"
            else
              "cap=${cfg.capacity}"
          }
          g layout assign -z ${box} -c "$cap" "$node"
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
            # owner too where it sets its own cors, as only an owner may
            g bucket allow --read --write ${lib.optionalString v.cors "--owner"} ${b} --key "$id" >/dev/null
            chown ${v.owner} ${state}/keys/${b}.env.new
            chmod 0400 ${state}/keys/${b}.env.new
            mv ${state}/keys/${b}.env.new ${state}/keys/${b}.env
            ${lib.optionalString v.cors ''
              # garage has no cli for cors: an s3 call with the bucket's key
              ( set -a; . ${state}/keys/${b}.env
                AWS_DEFAULT_REGION=us-east-1 ${pkgs.awscli2}/bin/aws --endpoint-url http://127.0.0.1:${toString cfg.port} \
                  s3api put-bucket-cors --bucket ${b} --cors-configuration ${lib.escapeShellArg (builtins.toJSON corsRule)} )
            ''}
          '') cfg.buckets
        )}
        # each other box's key here: written into what it copies, sealed to it
        ${lib.concatStrings (
          lib.mapAttrsToList (p: v: ''
            key peer-${p} > ${state}/from/${p}.env
            id=$(awk -F= '/ACCESS_KEY_ID/ { print $2 }' ${state}/from/${p}.env)
            # its copies, and the shared buckets it writes here
            for b in ${lib.concatStringsSep " " peerBuckets}; do
              g bucket allow --read --write "$b" --key "$id" >/dev/null
            done
            age -r ${lib.escapeShellArg v.recipient} -o ${sealed}/${p}.age.new ${state}/from/${p}.env
            chmod 0444 ${sealed}/${p}.age.new
            mv ${sealed}/${p}.age.new ${sealed}/${p}.age
          '') cfg.peers
        )}
      '';
    };

    systemd.services.dd-store-copy = copyUnit "all";
    systemd.services.dd-store-copy-recent = copyUnit "recent";
    systemd.services."dd-store-copy-final@" = copyUnit "final";
    systemd.timers.dd-store-copy = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "2min";
        OnUnitActiveSec = "30min";
      };
    };
    systemd.timers.dd-store-copy-recent = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1min";
        OnUnitActiveSec = "30s";
        AccuracySec = "5s";
      };
    };

    # the shared buckets (thanos' blocks, the nix cache): kept in one box's
    # store, which the others write with their own key to it. Here, each
    # one's key in ${state}/at/<bucket>.env, for its owner: this box's own
    # at home, and its key to the home's store elsewhere, handed to it
    # sealed as its copies' is (dd-store-copy), fetched again every hour
    systemd.services.dd-store-at = lib.mkIf (homed != { }) {
      description = "Keys to the shared buckets, at the stores that keep them";
      after = [ "dd-store-setup.service" ];
      wants = [ "dd-store-setup.service" ];
      wantedBy = [ "multi-user.target" ];
      restartIfChanged = true;
      path = [
        pkgs.coreutils
        pkgs.curl
        pkgs.age
        pkgs.findutils
      ]
      ++ cfg.agePlugins;
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        set -uo pipefail
        umask 077
        install -d -m 0755 ${state}/at
        install -d -m 0700 ${state}/remote
        failed=0
        put() { # put <bucket> <owner> <env file>
          cp "$3" ${state}/at/$1.env.new && chown "$2" ${state}/at/$1.env.new \
            && chmod 0400 ${state}/at/$1.env.new && mv ${state}/at/$1.env.new ${state}/at/$1.env
        }
        ${lib.concatStrings (
          lib.mapAttrsToList (
            b: v:
            if v.home == box then
              ''
                put ${b} ${v.owner} ${state}/keys/${b}.env || failed=1
              ''
            else
              ''
                f=${state}/remote/${v.home}.env
                if [ -z "$(find "$f" -mmin -60 2>/dev/null)" ]; then
                  if curl -fsS --max-time 30 ${cfg.peers.${v.home}.url}/_keys/${box}.age -o $f.age \
                    && age -d -i ${cfg.identity} $f.age > $f.new; then
                    mv $f.new $f
                  else
                    echo "${v.home}: no key from its store yet"
                  fi
                fi
                if [ -s "$f" ]; then put ${b} ${v.owner} "$f" || failed=1; else failed=1; fi
              ''
          ) homed
        )}
        # a home away is not this box failing: the timer asks again
        [ "$failed" = 0 ] || echo "some keys wait for the next run"
        exit 0
      '';
    };
    systemd.timers.dd-store-at = lib.mkIf (homed != { }) {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "5min";
        OnUnitActiveSec = "15min";
      };
    };

    # a key here for each box that joined on its own key, as for the
    # release's (dd-store-setup): written into the buckets copied, sealed
    # to the recipient it joined with; gone when it is gone from the
    # agreed state (only its key: what it copied here stays)
    systemd.services.dd-store-joined = {
      description = "Keys to this store for the boxes that joined on their own keys";
      after = [ "dd-store-setup.service" ];
      restartIfChanged = false;
      path = [
        pkgs.coreutils
        pkgs.gnugrep
        pkgs.gawk
        pkgs.age
        pkgs.curl
        pkgs.jq
      ]
      # a joined box's recipient is its TPM's
      ++ cfg.agePlugins;
      serviceConfig.Type = "oneshot";
      script = ''
        set -uo pipefail
        g() { ${garage} "$@"; }
        g status >/dev/null 2>&1 || exit 0
        st=$(curl -fsS --max-time 10 ${cfg.control}/state) || { echo "no agreed state yet"; exit 0; }
        umask 077
        key() {
          g key info "$1" >/dev/null 2>&1 || g key create "$1" >/dev/null
          g key info "$1" --show-secret | awk '
            /Key ID:/ { print "AWS_ACCESS_KEY_ID=" $NF }
            /Secret key:/ { print "AWS_SECRET_ACCESS_KEY=" $NF }'
        }
        now=$(printf '%s' "$st" | jq -r '.joined // {} | keys[]')
        for p in $now; do
          case "$p" in j[0-9a-f]*) ;; *) continue ;; esac
          [ -s ${sealed}/$p.age ] && continue
          recipient=$(printf '%s' "$st" | jq -r --arg p "$p" '.joined[$p].recipient')
          key peer-$p > ${state}/from/$p.env
          id=$(awk -F= '/ACCESS_KEY_ID/ { print $2 }' ${state}/from/$p.env)
          for b in ${lib.concatStringsSep " " copied}; do
            g bucket allow --read --write "$b" --key "$id" >/dev/null
          done
          if age -r "$recipient" -o ${sealed}/$p.age.new ${state}/from/$p.env; then
            chmod 0444 ${sealed}/$p.age.new
            mv ${sealed}/$p.age.new ${sealed}/$p.age
            echo "$p joined: its key here, sealed to it"
          else
            echo "$p: its recipient is not one age takes"
          fi
        done
        # a joined box gone from the agreed state: its key goes
        for f in ${state}/from/j*.env; do
          [ -e "$f" ] || continue
          p=$(basename "$f" .env)
          printf '%s\n' $now | grep -qx "$p" && continue
          g key delete --yes peer-$p >/dev/null 2>&1 || true
          rm -f "$f" ${sealed}/$p.age
          echo "$p is gone: its key here is too"
        done
        exit 0
      '';
    };
    systemd.timers.dd-store-joined = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "2min";
        OnUnitActiveSec = "5min";
      };
    };

    # its room, for the plan: how big, how much free, and what it keeps of
    # each bucket; and nearly full, it refuses other boxes' copies
    systemd.services.dd-store-facts = {
      description = "Say how much room this box's store has, and what it keeps";
      after = [ "dd-store-setup.service" ];
      restartIfChanged = false;
      path = [
        pkgs.coreutils
        pkgs.gawk
      ]
      ++ lib.optional (cfg.dataset != null) config.boot.zfs.package;
      serviceConfig.Type = "oneshot";
      script = ''
        set -uo pipefail
        g() { ${garage} "$@"; }
        # no store up yet: nothing to say, and nothing failed
        g status >/dev/null 2>&1 || exit 0
        avail=$(df -B1 --output=avail ${cfg.dataDir} | tail -1)
        out=$(mktemp)
        used=0
        for b in ${lib.concatStringsSep " " (builtins.attrNames cfg.buckets)}; do
          # read to the end: garage aborts on a pipe closed under it
          n=$(g bucket info $b 2>/dev/null | awk '
            BEGIN { u["B"] = 1; u["kiB"] = 2^10; u["MiB"] = 2^20; u["GiB"] = 2^30; u["TiB"] = 2^40 }
            /^Size:/ && !done { printf "%.0f", $2 * u[$3]; done = 1 }') || true
          [ -n "$n" ] || continue
          used=$((used + n))
          echo "dd_store_bucket_bytes{bucket=\"$b\"} $n" >> $out
        done
        ${
          if cfg.capacity == "auto" then
            ''
              # what it keeps, and what its pool has free less a fifth of
              # the pool: the rest is the box's other services'. The pool's
              # own size: df's is the dataset's, what it holds and what is
              # free, and on a box whose pool is nearly full (node2, 94%)
              # that kept back a fifth of almost nothing
              ${
                if cfg.dataset != null then
                  ''
                    pool=$(zfs get -Hp -o value used,available ${lib.head (lib.splitString "/" cfg.dataset)} | awk '{ s += $1 } END { printf "%.0f", s }')
                    [ -n "$pool" ] && [ "$pool" -gt 0 ] || pool=$(df -B1 --output=size ${cfg.dataDir} | tail -1)
                  ''
                else
                  "pool=$(df -B1 --output=size ${cfg.dataDir} | tail -1)"
              }
              spare=$((avail - pool / 5))
              [ "$spare" -lt 0 ] && spare=0
              cap=$((used + spare))
            ''
          else
            "cap=$(numfmt --from=iec ${cfg.capacity})"
        }
        free=$((cap - used))
        [ "$avail" -lt "$free" ] && free=$avail
        [ "$free" -lt 0 ] && free=0
        echo "dd_store_size_bytes $cap" >> $out
        echo "dd_store_free_bytes $free" >> $out
        install -d /var/lib/dd-facts
        chmod 0644 $out
        mv $out /var/lib/dd-facts/store.prom
        # a twentieth left: what other boxes send is refused here
        if [ "$free" -lt $((cap / 20)) ]; then
          [ -e ${full} ] || echo "store nearly full: $free of $cap bytes free; refusing copies"
          touch ${full}
        else
          rm -f ${full}
        fi
        exit 0
      '';
    };
    systemd.timers.dd-store-facts = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1min";
        OnUnitActiveSec = "1min";
      };
    };

    # other boxes reach this store, and their keys to it, by name on the
    # fleet's network
    # on a port of its own, which is all a box that joined on its own key
    # may reach of another box on the fleet's network (net/headscale.nix)
    networking.firewall.interfaces.commonty0.allowedTCPPorts = [ cfg.publicPort ];
    # a client signs no port when it is the scheme's own (the follow tests
    # serve the store on plain 80)
    services.nginx.appendHttpConfig = ''
      map $scheme:$server_port $dd_store_port {
        default ":$server_port";
        "http:80" "";
        "https:443" "";
      }
    '';
    services.nginx.virtualHosts."store.${box}.${base}" = {
      useACMEHost = base;
      onlySSL = true;
      listen = [
        {
          addr = "0.0.0.0";
          port = cfg.publicPort;
          ssl = true;
        }
        {
          addr = "[::]";
          port = cfg.publicPort;
          ssl = true;
        }
      ];
      locations."/".proxyPass = "http://127.0.0.1:${toString cfg.port}";
      # s3 signs its requests; no cookie of anyone's goes to the store
      locations."/".ddCookie = "";
      # the Host a box signed: the recommended headers send $host, which
      # drops the port, and since the store moved off 443 every copy in
      # failed "Invalid signature" (2026-10-20). The port it came in on, not
      # the client's $http_host (gixy refuses it), so the name is still
      # server_name's
      locations."/".recommendedProxySettings = false;
      # nearly full, nothing more from other boxes: what they send is
      # refused, what they read still served
      locations."/".extraConfig = ''
        set $dd_store_refuse "";
        if (-f ${full}) { set $dd_store_refuse "full"; }
        if ($request_method ~ ^(PUT|POST)$) { set $dd_store_refuse "''${dd_store_refuse}-write"; }
        if ($dd_store_refuse = "full-write") { return 507; }
        client_max_body_size 0;
        proxy_request_buffering off;
        proxy_set_header Host $host$dd_store_port;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
      '';
      locations."/_keys/".alias = "${sealed}/";
    };
  };
}
