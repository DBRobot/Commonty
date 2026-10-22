# The services the control plane switches (needs placed = true): their
# units on this box start only once /run/dd-placed/<service> exists, and
# dd-control puts it there and takes it away by the plan
# (servers/system/control/cmd/dd-control/placed.go). /run is empty at boot,
# so a placed service runs nowhere until the agreed state puts it somewhere.
# What this box carries - the placed services the plan may give it - is
# /etc/dd-placed/carries.json, and its heartbeat, so the plan only puts a
# service where it can run. A placed service built in here that it does not
# carry never runs here: its units are switched all the same, and no flag
# comes for them.
#
# A service's units are its own name and its needs' units, as they are on
# this box: a name ending in * every unit that starts so, one ending in @ a
# template and its instances; its timers, sockets and paths with them. A
# unit two services claim is the one's that claims it most closely (an
# exact name, then the longest prefix).
#
# A writer (needs lease) is also stopped by a timer of its own when its
# lease may have lapsed, so a dd-control that crashed or hangs does not
# leave it writing beside the next holder.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  needs = import ../../lib/needs.nix {
    inherit lib;
    on = config.dd.failover or false;
  };
  dir = "/run/dd-placed";
  isPlaced = s: needs.${s}.placed || needs.${s}.lease;
  roles = lib.filterAttrs (_: v: builtins.isAttrs v) (lib.importJSON ../../../fleet/roles.json);
  bring = rs: lib.unique (lib.concatMap (r: roles.${r}.services or [ ]) rs);
  # what this box carries: the placed services of the roles it is given
  # (fleet/boxes.json); and with failover, of the roles it is able to take
  # over (dd.control.roles), the writers and what goes with them - what
  # they need beside them, what needs them beside it - and what is spread
  # over every box that has it
  writers = lib.filter (s: needs.${s}.lease) (builtins.attrNames needs);
  local =
    s:
    lib.filter (d: needs ? ${d} && needs.${s}.base.depends.${d}.local) (
      builtins.attrNames needs.${s}.base.depends
    );
  follows =
    s:
    needs.${s}.lease
    || needs.${s}.spread
    || builtins.any (w: builtins.elem w writers) (local s)
    || builtins.any (w: builtins.elem s (local w)) writers;
  carried =
    if config.dd.placed.services != null then
      lib.filter (s: needs ? ${s} && isPlaced s) config.dd.placed.services
    else
      lib.filter isPlaced (
        bring config.dd.placed.roles ++ lib.filter follows (bring config.dd.control.roles)
      );

  # the units the box has, by name with their kind (foo.service, foo.timer)
  kinds = [
    "service"
    "timer"
    "socket"
    "path"
  ];
  kindOf = u: lib.last (lib.splitString "." u);
  baseOf = u: lib.removeSuffix ".${kindOf u}" u;
  boxUnits = lib.filter (u: builtins.elem (kindOf u) kinds && !(lib.hasInfix "/" u)) (
    builtins.attrNames config.systemd.units
  );
  # how closely a service's claim c takes unit base b: 0 not at all; an
  # exact name most, then the longest prefix (a template's own socket is
  # its name without the @)
  closeness =
    c: b:
    if lib.hasSuffix "*" c then
      (if lib.hasPrefix (lib.removeSuffix "*" c) b then 1 + lib.stringLength c else 0)
    else if lib.hasSuffix "@" c then
      (
        if lib.hasPrefix c b then
          1 + lib.stringLength c
        else if b == lib.removeSuffix "@" c then
          1 + lib.stringLength c
        else
          0
      )
    else if b == c then
      1000
    else
      0;
  claimsOf = s: [ s ] ++ needs.${s}.units;
  ownerOf =
    u:
    let
      b = baseOf u;
      scored = lib.concatMap (
        s:
        let
          n = lib.foldl' lib.max 0 (map (c: closeness c b) (claimsOf s));
        in
        lib.optional (n > 0) { inherit s n; }
      ) (builtins.attrNames needs);
      best = lib.foldl' (
        a: x: if a == null || x.n > a.n || (x.n == a.n && x.s < a.s) then x else a
      ) null scored;
    in
    if best == null then null else best.s;
  owned = lib.foldl' (
    a: u:
    let
      s = ownerOf u;
    in
    if s != null && isPlaced s then a // { ${s} = (a.${s} or [ ]) ++ [ u ]; } else a
  ) { } boxUnits;

  # a writer's units are written into the units themselves, as they always
  # were: its exact names, every one a service (read from the needs, not
  # config.systemd, which these conditions are part of). Every other placed
  # unit here is switched by a drop-in
  inline =
    s:
    lib.optionals needs.${s}.lease (
      lib.filter (u: !(lib.hasInfix "*" u || lib.hasSuffix "@" u)) needs.${s}.units
    );
  written = lib.genAttrs (lib.filter (s: inline s != [ ]) carried) inline;
  inlined = lib.concatMap (s: map (u: "${u}.service") written.${s}) (builtins.attrNames written);
  # what it carries: a service of which this box has some unit
  carries = lib.genAttrs (lib.filter (s: owned ? ${s} || written ? ${s}) carried) inline;

  # the units that copy a service's last state out on its handover
  # (dd.store.handover) run while it is handed over; the rest refuse to start
  # then, so nothing writes while or after that copy (placed.go marks it)
  handover = map (lib.removeSuffix ".service") (
    lib.concatLists (lib.attrValues (config.dd.store.handover or { }))
  );
  handing = import ../../lib/not-handing.nix { inherit pkgs; };
  when = svc: u: {
    unitConfig.ConditionPathExists = [ "${dir}/${svc}" ];
    serviceConfig = lib.optionalAttrs (!builtins.elem u handover) { ExecCondition = "${handing svc}"; };
  };
  # the rest of the placed services' units here: their timers, sockets and
  # paths, what their patterns and templates name, every unit of a service
  # placed elsewhere only
  dropped = lib.concatLists (
    lib.mapAttrsToList (
      s: us: map (u: { inherit s u; }) (lib.filter (u: !(builtins.elem u inlined)) us)
    ) owned
  );
  dropIn =
    { s, u }:
    let
      conf =
        "[Unit]\nConditionPathExists=${dir}/${s}\n"
        + lib.optionalString (
          kindOf u == "service" && !builtins.elem (baseOf u) handover
        ) "[Service]\nExecCondition=${handing s}\n";
      d = "\"$out\"/lib/systemd/system/${lib.escapeShellArg "${u}.d"}";
    in
    ''
      mkdir -p ${d}
      printf %s ${lib.escapeShellArg conf} > ${d}/dd-placed.conf
    '';
  dropIns = pkgs.runCommand "dd-placed-units" { } (
    "mkdir -p $out/lib/systemd/system\n" + lib.concatMapStrings dropIn dropped
  );
  # what dd-control starts when the flag goes up: of its units, what starts
  # at boot (wanted by a target; a template's instances by whatever starts
  # them), and a writer's every exact unit, as always
  bootTargets = [
    "multi-user.target"
    "timers.target"
    "sockets.target"
    "paths.target"
    "default.target"
  ];
  atBoot =
    u:
    let
      b = baseOf u;
      c = config.systemd."${kindOf u}s".${b} or { };
    in
    !(lib.hasSuffix "@" b) && builtins.any (t: builtins.elem t bootTargets) (c.wantedBy or [ ]);
  service = u: "${u}.service";
  # what dd-control stops when it goes down: every unit, a template's as
  # the pattern of its instances
  stopsOf =
    s:
    lib.unique (
      map service carries.${s}
      ++ map (u: if lib.hasSuffix "@" (baseOf u) then "${baseOf u}*.${kindOf u}" else u) (
        owned.${s} or [ ]
      )
    );
  startsOf = s: lib.unique (map service carries.${s} ++ lib.filter atBoot (owned.${s} or [ ]));
in
{
  options.dd.placed.services = lib.mkOption {
    type = lib.types.nullOr (lib.types.listOf lib.types.str);
    default = null;
    description = "the placed services this box carries, as the release gives them (nix/lib/fleet-placement.nix carries: of what it is built with, what its capability lets it run). Null: from its roles (dd.placed.roles), as a test's box has them";
  };
  options.dd.placed.roles = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = config.dd.control.roles;
    defaultText = "dd.control.roles";
    description = "the roles whose placed services this box carries: its own in fleet/boxes.json. With failover it carries as well, of the roles it is able to take over (dd.control.roles), the writers and what goes with them";
  };

  config = lib.mkIf config.dd.control.enable {
    # a name that is no unit would be made one by its condition, with
    # nothing to run, and fail the start of all the rest
    assertions = lib.mapAttrsToList (s: units: {
      assertion = lib.all (u: config.systemd.services.${u}.serviceConfig ? ExecStart) units;
      message = "placed service ${s}: each exact name a writer's needs list must be a service this box defines (a timer of the same name comes with it; * and @ name the rest)";
    }) written;
    environment.etc."dd-placed/carries.json".text = builtins.toJSON (
      lib.genAttrs (builtins.attrNames carries) stopsOf
    );
    # what to start of each, where that is not every unit it has
    environment.etc."dd-placed/starts.json".text = builtins.toJSON (
      lib.genAttrs (builtins.attrNames carries) startsOf
    );
    systemd.packages = lib.optional (dropped != [ ]) dropIns;
    systemd.tmpfiles.rules = [ "d ${dir} 0755 root root -" ];
    systemd.services = lib.mkMerge (
      [
        {
          dd-placed-fence = {
            description = "Stop a writer whose lease may have lapsed";
            restartIfChanged = false;
            path = [
              pkgs.jq
              pkgs.coreutils
              pkgs.systemd
            ];
            serviceConfig.Type = "oneshot";
            script = builtins.readFile ./placed-fence.sh;
          };
        }
      ]
      ++ lib.mapAttrsToList (s: units: lib.genAttrs units (when s)) written
    );
    systemd.timers.dd-placed-fence = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "2s";
        OnUnitActiveSec = "2s";
        AccuracySec = "500ms";
      };
    };
  };
}
