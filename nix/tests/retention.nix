# Nothing deleted lasts past 90 days on any box, wherever a box's config
# says how long it keeps something (docs/dev/servers.md, Retention)
{
  pkgs,
  self,
  lib,
  ...
}:
let
  limitMin = 90 * 24 * 60;
  minutes = {
    frequent = 15;
    hourly = 60;
    daily = 24 * 60;
    weekly = 7 * 24 * 60;
    monthly = 31 * 24 * 60;
    yearly = 366 * 24 * 60;
  };
  over = m: m > limitMin;
  days = m: toString (m / (24 * 60));

  snapshots =
    name: c:
    let
      s = c.services.zfs.autoSnapshot;
      running = n: c.systemd.timers."zfs-snapshot-${n}".enable or true;
      kept = n: s.${n} * minutes.${n};
      levels = builtins.filter running [
        "frequent"
        "hourly"
        "daily"
        "weekly"
        "monthly"
      ];
      trim = (c.dd.zfs.snapshotMaxDays or 1000) * minutes.daily;
    in
    lib.optionals s.enable (
      map (n: "${name}: ${n} snapshots keep ${days (kept n)} days") (
        builtins.filter (n: over (kept n)) levels
      )
      ++ lib.optional (over trim) "${name}: dd-snapshot-trim keeps snapshots ${days trim} days"
    );

  # restic's --keep-<unit> N and --keep-within <N>d, as minutes
  resticKeep =
    opt:
    let
      m = builtins.match "--keep-(daily|weekly|monthly|yearly|hourly) ([0-9]+)" opt;
      w = builtins.match "--keep-within(-[a-z]+)? ([0-9]+)d" opt;
    in
    if m != null then
      lib.toInt (builtins.elemAt m 1) * minutes.${builtins.elemAt m 0}
    else if w != null then
      lib.toInt (builtins.elemAt w 1) * minutes.daily
    else
      null;
  restic =
    name: c:
    lib.concatLists (
      lib.mapAttrsToList (
        repo: b:
        let
          keeps = map resticKeep b.pruneOpts;
          longest = lib.foldl' lib.max 0 (builtins.filter (k: k != null) keeps);
        in
        if b.pruneOpts == [ ] then
          [ "${name}: restic ${repo} is never pruned" ]
        else if builtins.elem null keeps then
          [ "${name}: restic ${repo} has a keep option this check cannot read: ${toString b.pruneOpts}" ]
        else
          lib.optional (over longest) "${name}: restic ${repo} keeps ${days longest} days"
      ) c.services.restic.backups
    );

  vaultwarden =
    name: c:
    let
      v = c.services.vaultwarden;
      env = v.config;
      read =
        key:
        if env ? ${key} && env.${key} != null then
          (
            if builtins.isInt env.${key} then
              [
                "ok"
                env.${key}
              ]
            else
              [
                "bad"
                0
              ]
          )
        else
          [
            "unset"
            0
          ];
      check =
        key:
        let
          r = read key;
          kind = builtins.elemAt r 0;
          n = builtins.elemAt r 1;
        in
        if kind == "unset" then
          [ "${name}: vaultwarden ${key} is unset, which keeps forever" ]
        else if kind == "bad" then
          [ "${name}: vaultwarden ${key} is not a number of days" ]
        else
          lib.optional (n > 90) "${name}: vaultwarden ${key} is ${toString n} days";
    in
    lib.optionals v.enable (check "TRASH_AUTO_DELETE_DAYS" ++ check "EVENTS_DAYS_RETAIN");

  journald =
    name: c:
    let
      m = builtins.match ".*MaxRetentionSec=([0-9]+)day.*" (
        builtins.replaceStrings [ "\n" ] [ " " ] c.services.journald.extraConfig
      );
    in
    if m == null then
      [ "${name}: journald has no MaxRetentionSec in days" ]
    else
      lib.optional (lib.toInt (builtins.head m) > 90) "${name}: journald keeps ${builtins.head m} days";

  nginxLog =
    name: c:
    let
      r = c.services.logrotate.settings.nginx or null;
      per = minutes.${(r.frequency or "weekly")} or null;
    in
    lib.optionals (c.services.nginx.enable && r != null) (
      if per == null then
        [ "${name}: nginx's log rotates ${r.frequency}, which this check cannot read" ]
      else
        lib.optional (over (r.rotate * per)) "${name}: nginx's log keeps ${days (r.rotate * per)} days"
    );

  # museum has no setting for either: its trash empties after a constant
  # 30 days and a deleted file's objects go 45 days after that; read from
  # the source each box runs
  ente = lib.filterAttrs (
    _: sys: sys.config.services.ente.api.enable or false
  ) self.nixosConfigurations;
  enteScript = lib.concatStrings (
    lib.mapAttrsToList (name: sys: ''
      src=${sys.config.services.ente.api.package.src}/server/pkg/repo
      trash=$(sed -n 's/^\s*TrashDurationInDays = \([0-9]*\)$/\1/p' $src/trash.go)
      queue=$(sed -n 's|^\s*DeleteObjectQueue: *\([0-9]*\) \* 24 \* 60,.*|\1|p' $src/queue.go)
      if [ -z "$trash" ] || [ -z "$queue" ]; then
        echo "${name}: museum's trash or object-deletion delay is no longer where this check reads it" >&2; bad=1
      elif [ $((trash + queue)) -gt 90 ]; then
        echo "${name}: museum keeps a deleted photo $trash + $queue days" >&2; bad=1
      fi
    '') ente
  );

  # what someone who left keeps until it is deleted (servers/gate/src/leaving.rs)
  leaving =
    name: c:
    let
      w = c.dd.verify.leaveWindowSeconds or null;
    in
    lib.optional (
      w != null && w > limitMin * 60
    ) "${name}: someone who left keeps their things ${toString (w / 86400)} days";

  problems = lib.concatLists (
    lib.mapAttrsToList (
      name: sys:
      let
        c = sys.config;
      in
      snapshots name c
      ++ restic name c
      ++ vaultwarden name c
      ++ journald name c
      ++ nginxLog name c
      ++ leaving name c
    ) self.nixosConfigurations
  );
in
pkgs.runCommand "retention-90-days" { } (
  ''
    bad=0
  ''
  + lib.optionalString (problems != [ ]) ''
    printf '%s\n' ${lib.escapeShellArgs problems} >&2; bad=1
  ''
  + enteScript
  + ''
    [ $bad = 0 ] || exit 1
    echo 'nothing on any box is kept past 90 days'
    touch $out
  ''
)
