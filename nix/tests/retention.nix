# Anything deleted is gone from every box and every backup within 90 days:
# the first place's window (a trash, someone who left), then the backups,
# then the snapshots under them, summed (docs/dev/servers.md, Retention)
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
  longestOf = l: lib.foldl' lib.max 0 l;

  # a window in the Rust, `<NAME>: u64 = <n> * 24 * 3600`, as minutes
  rustDays =
    file: name:
    let
      m = builtins.match ".*${name}: u64 = ([0-9]+) \\* 24 \\* 3600;.*" (
        builtins.replaceStrings [ "\n" ] [ " " ] (builtins.readFile file)
      );
    in
    if m == null then
      throw "retention: ${name} is no longer where this check reads it in ${toString file}"
    else
      lib.toInt (builtins.head m) * minutes.daily;
  libraryTrash = rustDays ../../servers/library/src/dav.rs "TRASH_SECS";
  leaveDefault = rustDays ../../servers/gate/src/leaving.rs "pub const WINDOW";

  snapshotsKeep =
    c:
    let
      s = c.services.zfs.autoSnapshot;
      running = n: c.systemd.timers."zfs-snapshot-${n}".enable or true;
      levels = builtins.filter running [
        "frequent"
        "hourly"
        "daily"
        "weekly"
        "monthly"
      ];
    in
    if s.enable then
      lib.min ((c.dd.zfs.snapshotMaxDays or 1000) * minutes.daily) (
        longestOf (map (n: s.${n} * minutes.${n}) levels)
      )
    else
      0;

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

  # restic's --keep-<unit> N and --keep-within <N>d, as minutes: N whole
  # units back, plus the one under way
  resticKeep =
    opt:
    let
      m = builtins.match "--keep-(daily|weekly|monthly|yearly|hourly) ([0-9]+)" opt;
      w = builtins.match "--keep-within(-[a-z]+)? ([0-9]+)d" opt;
    in
    if m != null then
      (lib.toInt (builtins.elemAt m 1) + 1) * minutes.${builtins.elemAt m 0}
    else if w != null then
      lib.toInt (builtins.elemAt w 1) * minutes.daily
    else
      null;
  resticLongest =
    c:
    longestOf (
      lib.mapAttrsToList (
        _: b: longestOf (builtins.filter (k: k != null) (map resticKeep b.pruneOpts))
      ) c.services.restic.backups
    );

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
          [ ];
    in
    lib.optionals v.enable (check "TRASH_AUTO_DELETE_DAYS" ++ check "EVENTS_DAYS_RETAIN");
  vaultwardenDays =
    c:
    let
      env = c.services.vaultwarden.config;
      d = k: if builtins.isInt (env.${k} or null) then env.${k} * minutes.daily else 0;
    in
    if c.services.vaultwarden.enable then
      lib.max (d "TRASH_AUTO_DELETE_DAYS") (d "EVENTS_DAYS_RETAIN")
    else
      0;

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
      snap=${toString (snapshotsKeep sys.config / minutes.daily)}
      left=${toString (leftDays sys.config)}
      src=${sys.config.services.ente.api.package.src}/server
      trash=$(sed -n 's/^\s*TrashDurationInDays = \([0-9]*\)$/\1/p' $src/pkg/repo/trash.go)
      queue=$(sed -n 's|^\s*DeleteObjectQueue: *\([0-9]*\) \* 24 \* 60,.*|\1|p' $src/pkg/repo/queue.go)
      account=$(sed -n 's|.*stage_schedule_time .*now_utc_micro_seconds() + (\([0-9]*\) \* 24::BIGINT.*|\1|p' $src/migrations/59_delete_data.up.sql)
      if [ -z "$trash" ] || [ -z "$queue" ] || [ -z "$account" ]; then
        echo "${name}: museum's trash, account or object-deletion delay is no longer where this check reads it" >&2; bad=1
      else
        if [ $((trash + queue + snap)) -gt 90 ]; then
          echo "${name}: museum keeps a deleted photo $trash + $queue days, then snapshots $snap" >&2; bad=1
        fi
        # someone who left: the window and its hour, then museum's account delay
        if [ $((left + 1 + account + queue + snap)) -gt 90 ]; then
          echo "${name}: the Photos of someone who left go after $left + 1 + $account + $queue + $snap days" >&2; bad=1
        fi
      fi
    '') ente
  );

  leftMinutes =
    c:
    let
      w = c.dd.verify.leaveWindowSeconds or null;
    in
    if w == null then leaveDefault else w / 60;
  leftDays = c: (leftMinutes c + minutes.daily - 1) / minutes.daily;

  # the first window, then a backup of the last copy, then a snapshot of
  # that backup
  chain =
    name: c:
    let
      first = {
        "the library trash" = libraryTrash;
        "someone who left" = leftMinutes c;
        "Passwords" = vaultwardenDays c;
      };
      after = resticLongest c + snapshotsKeep c;
    in
    lib.concatLists (
      lib.mapAttrsToList (
        what: m:
        lib.optional (over (m + after))
          "${name}: ${what} keeps ${days m} days, then backups and snapshots ${days after}: ${days (m + after)} days"
      ) first
    );

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
      ++ chain name c
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
    echo 'anything deleted is gone from every box and backup within 90 days'
    touch $out
  ''
)
