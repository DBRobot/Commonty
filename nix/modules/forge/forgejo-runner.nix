{
  config,
  pkgs,
  lib,
  ddScript,
  ...
}:
let
  base = config.dd.domain;
  name = config.networking.hostName;
  restartScript =
    mode:
    pkgs.writeShellScript "runner-restart-${mode}" (
      ddScript ./runner-restart.sh {
        MODE = mode;
        UNIT = "gitea-runner-${name}.service";
        PATH = lib.makeBinPath [
          config.systemd.package
          pkgs.coreutils
          pkgs.gnugrep
        ];
      }
    );
  cfg = config.dd.runner;
  fixed = cfg.capacity != null;
  # until dd-service-memory has measured them: what the needs say the
  # services this box's roles bring hold, the runners aside
  needs = import ../../lib/needs.nix { inherit lib; };
  sizes =
    (import ../../lib/capability.nix {
      inherit lib needs;
      boxes = { };
      policy = { };
    }).sizes;
  roles = lib.filterAttrs (_: v: builtins.isAttrs v) (lib.importJSON ../../../fleet/roles.json);
  brought = lib.unique (
    lib.concatMap (r: roles.${r}.services or [ ]) (config.dd.control.roles or [ ])
    ++ lib.attrNames (lib.filterAttrs (_: n: n.everyBox) needs)
  );
  fallback = lib.foldl' (a: s: a + sizes.${s}.memory) 0.0 (
    lib.filter (
      s:
      needs ? ${s}
      && !(builtins.elem s [
        "forgejo-runner"
        "members-runner"
      ])
    ) brought
  );
  members = config.dd.membersRunner or { };
  membersGiB =
    if (members.tokenFile or null) != null then members.capacity * members.jobMemoryGiB else 0;
  configFile = (pkgs.formats.yaml { }).generate "config.yaml" (
    config.services.gitea-actions-runner.instances.${name}.settings
  );
  live = "/var/lib/gitea-runner/${name}/config.yaml";
in
{
  imports = [ ../box/work-memory.nix ];

  # what one job takes, measured (node1, 2026-10-20): its runner at 12.1G
  # with two jobs, the biggest VM test's qemu at 5.3G; and builds use what
  # threads they are given (8 threads were 85% busy at the 90th percentile
  # with two jobs on each box)
  options.dd.runner.jobThreads = lib.mkOption {
    type = lib.types.ints.positive;
    default = 4;
  };
  options.dd.runner.jobMemoryGiB = lib.mkOption {
    type = lib.types.ints.positive;
    default = 6;
  };
  # a release build beside CI (dd-build: 6.2G on node1, 5.6G on node2 at peak)
  options.dd.runner.buildMemoryGiB = lib.mkOption {
    type = lib.types.ints.unsigned;
    default = 6;
  };
  options.dd.runner.capacity = lib.mkOption {
    type = lib.types.nullOr lib.types.ints.positive;
    default = null;
    description = "Jobs this box runs at once. Null (the default): reckoned when the runner starts, from the box's threads and memory and what its services hold (runner-capacity.sh)";
  };

  config = {
    # reads plaintext: every box it runs on can read the code its jobs run
    # (nix/modules/box/box.nix)
    dd.box.plaintext = [ "forgejo-runner (runs contributed code)" ];
    # One runner, on this box, in host mode: a job runs as the runner user with
    # nix on the path and builds straight into this box's store. So the closure
    # a green check just built is already here, and a deploy is a switch.
    #
    # Host mode means a job is code running on this box as forgejo-runner.
    # So the host runners take the admin's repositories and no one else's:
    # the forge's database moves any runner registered for every repository
    # to the admin's scope, whatever token registered it (dd-forge-rules in
    # nix/modules/forge/forgejo.nix). A pull request from someone else's
    # fork runs only once the admin approves it (Forgejo's own rule).
    services.gitea-actions-runner = {
      package = pkgs.forgejo-runner;
      instances.${name} = {
        enable = true;
        inherit name;
        url = "https://git.${base}";
        # a registration token, minted once with `forgejo actions
        # generate-runner-token --scope <admin>` and kept in sops; the runner
        # registers on first start and keeps its own credential in its state
        # dir after
        tokenFile = config.sops.templates."forgejo-runner.env".path;
        # nix: the job runs on a box with nix. And the box's own name, for a
        # job that must run on this box and no other: each box building every
        # box's system (entry_point.yml, build_host), so that what it later
        # vouches for it has built itself
        labels = [
          "nix:host"
          "${name}:host"
        ];
        settings.runner = {
          # 0 is a stand-in that runner-capacity.sh replaces at every start
          capacity = if fixed then cfg.capacity else 0;
          # a stop lets the jobs under way finish first: the runner takes no
          # new one and exits when the last is done. At the default (0s) a
          # stop killed every job it held. Its own limit on a job, so no job
          # is ever cut short for it (TimeoutStopSec below)
          shutdown_timeout = "3h";
        };
        hostPackages = with pkgs; [
          bash
          coreutils
          curl
          gawk
          gitMinimal
          gnused
          gnugrep
          gnutar
          gzip
          zstd
          nodejs # javascript actions (checkout) need it
          wget
          nix
          openssh
        ];
      };
    };

    # Not the module's DynamicUser: systemd mounts a dynamic user's state
    # directory noexec (idmapped), and a host-mode job builds and runs things
    # there - every cargo build script died with "Permission denied". A plain
    # system user gets an ordinary directory.
    users.users.forgejo-runner = {
      isSystemUser = true;
      group = "forgejo-runner";
      home = "/var/lib/gitea-runner";
    };
    users.groups.forgejo-runner = { };
    # cargo's shared target dir, which nothing let go of (5.5G on node1):
    # what sits untouched for two weeks goes, and cargo rebuilds an output
    # it finds missing. Not the git caches beside it (act's actions, nix's
    # fetches): aging files inside a git directory leaves it a broken
    # repository, which failed CI on node2.
    systemd.tmpfiles.rules = [
      "e /var/lib/gitea-runner/cargo-target - - - 14d"
    ];
    # (the nixos module registers again by itself when the token or the
    # labels change, so a new scope reaches the forge on the next start; a
    # changed address is ours to notice, runner-address.sh)
    # no flake evaluation cache on a runner: it remembers derivation paths
    # that a later collection removed and then refuses them ("path ...drv
    # is not valid"); a job evaluates from scratch, which costs seconds
    nix.settings.eval-cache = false;

    # a job's own processes (evaluation, lint, cargo) run in this unit, under
    # the work slice's limit with the builds. A process the kernel kills
    # fails its job and nothing else: under systemd's default (stop) a kill
    # shut the runner down and every job it held with it (node2, 2026-10-20)
    dd.work.units."gitea-runner-${name}".oomPolicy = "continue";
    systemd.services."gitea-runner-${name}" = {
      path = [ pkgs.curl ];
      # A switch never stops it: a release that changed the runner stopped
      # it under the next pull request's CI and failed the run (release 331,
      # run 786). The new config waits for dd-runner-restart, below.
      restartIfChanged = false;
      serviceConfig = {
        DynamicUser = lib.mkForce false;
        # A stop is a drain: the signal goes to the runner alone (systemd
        # would otherwise send it to every job's processes too: "signal:
        # terminated"), and it may take the runner's job limit (3h) and a
        # little to let them end
        KillMode = "mixed";
        TimeoutStopSec = "190min";
        User = lib.mkForce "forgejo-runner";
        Group = "forgejo-runner";
        # the wait for the forge (its certificate arrives after a switch)
        TimeoutStartSec = "10min";
        ExecStartPre = lib.mkBefore (
          [
            # as root: which unit definition this run started from
            "+${restartScript "started"}"
            (pkgs.writeShellScript "runner-address" (
              ddScript ./runner-address.sh {
                NAME = name;
                URL = "https://git.${base}";
              }
            ))
          ]
          ++ lib.optional (!fixed) (
            pkgs.writeShellScript "runner-capacity" (
              ddScript ./runner-capacity.sh {
                JOB_THREADS = cfg.jobThreads;
                JOB_GIB = cfg.jobMemoryGiB;
                BUILD_GIB = cfg.buildMemoryGiB;
                MEMBERS_GIB = membersGiB;
                FALLBACK_GIB = toString (builtins.ceil fallback);
                FACTS = "/var/lib/dd-facts/services.prom";
                IN = configFile;
                OUT = live;
              }
            )
          )
        );
        ExecStart = lib.mkIf (!fixed) (
          lib.mkForce "${lib.getExe config.services.gitea-actions-runner.package} daemon --config ${live}"
        );
      };
    };

    # The runner restarts into its new config once it holds no job: every
    # minute this compares the unit on disk with the one the runner started
    # from, and restarts it only when no job's process is left in it. A job
    # taken in the instant between is drained, not killed (shutdown_timeout).
    systemd.services.dd-runner-restart = {
      description = "Restart the runner into its new config once it holds no job";
      restartIfChanged = false;
      serviceConfig = {
        Type = "oneshot";
        ExecStart = restartScript "check";
      };
    };
    systemd.timers.dd-runner-restart = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1min";
        OnUnitActiveSec = "1min";
      };
    };

    sops.secrets.forgejo-runner-token = { };
    # read by systemd itself, before the unit drops to its dynamic user, so the
    # file can stay root-only
    sops.templates."forgejo-runner.env".content = ''
      TOKEN=${config.sops.placeholder.forgejo-runner-token}
    '';
  };
}
