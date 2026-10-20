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
in
{
  imports = [ ../box/work-memory.nix ];

  # what one job takes: the biggest VM test is three VMs of 1.5G, and a
  # build takes every thread it is given
  options.dd.runner.jobThreads = lib.mkOption {
    type = lib.types.ints.positive;
    default = 4;
  };
  options.dd.runner.jobMemoryGiB = lib.mkOption {
    type = lib.types.ints.positive;
    default = 6;
  };
  options.dd.runner.capacity = lib.mkOption {
    type = lib.types.ints.positive;
    default =
      let
        gib = config.dd.box.memoryBytes / (1024 * 1024 * 1024);
      in
      lib.max 1 (
        lib.min (config.dd.box.threads / config.dd.runner.jobThreads) (gib / config.dd.runner.jobMemoryGiB)
      );
    description = "Jobs this box runs at once: as many as its threads and memory hold (box.json runnerCapacity overrides)";
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
        settings.runner.capacity = config.dd.runner.capacity;
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
      serviceConfig = {
        DynamicUser = lib.mkForce false;
        User = lib.mkForce "forgejo-runner";
        Group = "forgejo-runner";
        # the wait for the forge (its certificate arrives after a switch)
        TimeoutStartSec = "10min";
        ExecStartPre = lib.mkBefore [
          (pkgs.writeShellScript "runner-address" (
            ddScript ./runner-address.sh {
              NAME = name;
              URL = "https://git.${base}";
            }
          ))
        ];
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
