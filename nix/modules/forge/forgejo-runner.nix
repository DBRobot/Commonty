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
  options.dd.runner.fetchToken = lib.mkOption {
    type = lib.types.functionTo lib.types.package;
    description = "kind -> the script that fetches this box's sealed registration token of that kind";
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
        # box (runner-keys.nix), fetched before the runner starts; the runner
        # registers on first start and keeps its own credential in its state
        # dir after. Nobody mints it by hand
        tokenFile = "/var/lib/dd-runner-token/host.env";
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
      # where the fetched tokens are kept: root's alone, read by systemd
      # itself before the unit drops to its user
      "d /var/lib/dd-runner-token 0700 root root -"
    ];
    # (the nixos module registers again by itself when the token or the
    # labels change, so a new scope reaches the forge on the next start; a
    # changed address is ours to notice, runner-address.sh)
    # no flake evaluation cache on a runner: it remembers derivation paths
    # that a later collection removed and then refuses them ("path ...drv
    # is not valid"); a job evaluates from scratch, which costs seconds
    nix.settings.eval-cache = false;

    systemd.services."gitea-runner-${name}" = {
      path = [ pkgs.curl ];
      serviceConfig = {
        DynamicUser = lib.mkForce false;
        User = lib.mkForce "forgejo-runner";
        Group = "forgejo-runner";
        # the wait for the forge (its certificate arrives after a switch)
        TimeoutStartSec = "10min";
        ExecStartPre = lib.mkBefore [
          "+${fetchToken "host"}"
          (pkgs.writeShellScript "runner-address" (
            ddScript ./runner-address.sh {
              NAME = name;
              URL = "https://git.${base}";
            }
          ))
        ];
      };
    };

    # the members' runner fetches its own the same way (members-runner.nix)
    dd.runner.fetchToken = fetchToken;
  };
}
