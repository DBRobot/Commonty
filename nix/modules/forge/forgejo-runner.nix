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
  configFile = (pkgs.formats.yaml { }).generate "config.yaml" (
    config.services.gitea-actions-runner.instances.${name}.settings
  );
  live = "/var/lib/gitea-runner/${name}/config.yaml";
  instance = config.services.gitea-actions-runner.instances.${name};
  # this box's registration token for a runner, from the forge's box,
  # sealed to this box (runner-keys.nix, runner-token.sh)
  fetchToken =
    kind:
    pkgs.writeShellScript "runner-token-${kind}" (
      ddScript ./runner-token.sh {
        PATH = lib.makeBinPath [
          pkgs.curl
          pkgs.age
          pkgs.age-plugin-tpm
          pkgs.gnugrep
          pkgs.coreutils
        ];
        URL = "https://git.${base}";
        NAME = name;
        KIND = kind;
        OUT = "/var/lib/dd-runner-token/${kind}.env";
        IDENTITY = "/var/lib/dd/secrets-key.txt";
      }
    );
in
{
  imports = [
    ../box/work-memory.nix
    ./runner-restart.nix
  ];

  # the threads a typical job keeps busy, its own processes and the builds
  # it sends the nix daemon together: about one on node1 (2026-10-26, the
  # work slice's CPU time over the jobs' hours: 3.4 CPU-hours in 3.7 job
  # hours), so two leaves room for its bursts. The daemon's max-jobs
  # (roles/core.nix) caps the builds of all of them together, and the
  # busy cores at node1's 90th percentile are its model server's, not CI's
  options.dd.runner.jobThreads = lib.mkOption {
    type = lib.types.ints.positive;
    default = 2;
  };
  # what a typical job holds in the work slice, its own processes and the
  # builds it sends the nix daemon together (node2 at one job, 2026-10-22,
  # 44 minutes: 2.3G at the median, 3.9G at the 90th percentile). Not the
  # rare big one (CI's evaluation at 7G, a VM test's VMs at 5.6G, biome at
  # 10.5G): past the slice's limit the kernel kills that job alone
  options.dd.runner.jobMemoryGiB = lib.mkOption {
    type = lib.types.ints.positive;
    default = 4;
  };
  options.dd.runner.fetchToken = lib.mkOption {
    type = lib.types.functionTo lib.types.package;
    description = "kind -> the script that fetches this box's sealed registration token of that kind";
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
    # So the host runners take the fleet's own repositories and no one
    # else's: the forge's database moves any runner registered for every
    # repository to the fleet repository's owner's scope, whatever token
    # registered it (dd-forge-rules in nix/modules/forge/forgejo.nix). A pull
    # request from someone else's fork runs only once the repository's own
    # owner approves it (Forgejo's own rule); no person is the site admin.
    services.gitea-actions-runner = {
      package = pkgs.forgejo-runner;
      instances.${name} = {
        enable = true;
        inherit name;
        url = "https://git.${base}";
        # a registration token, made by the forge's box and sealed to this
        # box (runner-keys.nix), fetched by runner-start.sh once the forge
        # answers; the runner registers on first start and keeps its own credential in its state
        # dir after. Nobody mints it by hand
        tokenFile = "/var/lib/dd-runner-token/host.env";
        # nix: the job runs on a box with nix. And the box's own name, for a
        # job that must run on this box and no other: each box building every
        # box's system (entry_point.yml, build_systems), so that what it later
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
      # where the fetched tokens are kept: root's alone, read by
      # runner-start.sh before it drops to its user
      "d /var/lib/dd-runner-token 0700 root root -"
      # there before the first fetch: systemd reads a unit's environment
      # file (the module's tokenFile) before it starts, and a missing one
      # fails the start
      "f /var/lib/dd-runner-token/host.env 0600 root root - TOKEN="
      "f /var/lib/dd-runner-token/members.env 0600 root root - TOKEN="
    ];
    # (runner-start.sh registers again when the token, the labels or the
    # forge's address change, so a new scope reaches the forge on the next
    # start)
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
      # a forge that is down (it is on another box, or between boxes) is
      # waited out: never a failed unit, and never a switch held for it
      startLimitIntervalSec = 0;
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
        Restart = lib.mkForce "always";
        RestartSec = lib.mkForce 5;
        RestartSteps = 6;
        RestartMaxDelaySec = "5min";
        # nothing before the runner that needs the network: the module's
        # registration moves into runner-start.sh, after the forge answers
        ExecStartPre = lib.mkForce (
          lib.optional (!fixed) (
            pkgs.writeShellScript "runner-capacity" (
              ddScript ./runner-capacity.sh {
                JOB_THREADS = cfg.jobThreads;
                JOB_GIB = cfg.jobMemoryGiB;
                FALLBACK_GIB = toString (builtins.ceil fallback);
                FACTS = "/var/lib/dd-facts/services.prom";
                IN = configFile;
                OUT = live;
              }
            )
          )
        );
        # "!": it starts as root, for its token alone (runner-start.sh
        # fetches it once the forge answers), and drops to its user before
        # it registers
        ExecStart = lib.mkForce (
          "!"
          + pkgs.writeShellScript "runner-start" (
            ddScript ./runner-start.sh {
              NAME = name;
              URL = instance.url;
              LABELS = lib.concatStringsSep "," instance.labels;
              RUNNER = lib.getExe config.services.gitea-actions-runner.package;
              CONFIG = if fixed then configFile else live;
              FETCH = fetchToken "host";
              TOKEN_FILE = instance.tokenFile;
              SETPRIV = "${pkgs.util-linux}/bin/setpriv";
              RUN_AS = "forgejo-runner";
              RUN_GROUPS = "";
            }
          )
        );
      };
    };

    # A switch never stops it; it restarts into its new config once it
    # holds no job's process (runner-restart.nix). A job taken in the
    # instant between is drained, not killed (shutdown_timeout). Not past
    # its job limit and a little: then it drains and restarts all the same.
    dd.runnerRestart.units."gitea-runner-${name}" = {
      job = "cgroup";
      limit = (180 + 10) * 60;
    };

    # the members' runner fetches its own the same way (members-runner.nix)
    dd.runner.fetchToken = fetchToken;
  };
}
