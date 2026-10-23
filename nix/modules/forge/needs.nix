# What the forge needs of a box (types: nix/lib/needs-type.nix)
{
  forgejo = {
    what = "git: repositories, issues, pull requests, CI runs, releases";
    # public repositories only, which members push and clone
    traffic = {
      class = "medium";
      upMbps = 5;
    };
    alwaysOn = true;
    grows = [ "code" ];
    lease = true;
    # its repositories and files, copied out of a snapshot (forgejo.nix)
    buckets.forge = { };
    # the forge, and its public repositories for anyone (door.nix)
    hosts = [
      "git"
      "code"
    ];
    units = [
      "forgejo"
      "forgejo-admin"
      "dd-forge-copy" # its repositories into the store, with failover on (follow.nix)
      "dd-forge-handover" # the same, handed over: fails when it did not copy (follow.nix)
      "forgejo-protection"
      "forgejo-secrets"
      "dd-forge-dirs" # its directories on its dataset, once mounted
      "dd-ci-cancel"
      "dd-ci-resume"
      "dd-forge-notify"
      "dd-usage-forgejo"
      "dd-forge-rules"
      "dd-forget-forge" # its share of deleting whoever left
      "dd-runner-keys" # the runners' registration tokens, sealed to each runner box (runner-keys.nix)
    ];
    # its repositories are on one box's disk, beside its database
    instances = 1;
    base = {
      # repositories, LFS, release files of up to 400M each: overestimated
      requires.disk.size.min = "20G";
      # node1, 2026-10-04: peak; its dataset (and 70M in postgresql)
      uses = {
        memory = "151M";
        storage = "400M";
      };
      network = {
        outbound = [ "github.com" ]; # the actions CI checks out
        inbound = "network"; # signing in and pushing
      };
      depends = {
        postgresql.local = true;
        nginx.local = true;
        dd-verify.local = true; # sign-in in front of every page
        sshd.local = true; # git over ssh is the box's own sshd
      };
      privacy = "plaintext"; # repositories are kept as they are pushed
    };
  };

  forgejo-public = {
    what = "the public repositories' pages, to anyone";
    alwaysOn = true;
    base = {
      uses = {
        memory = "0";
        storage = "0";
      }; # forgejo's own process
      network = {
        outbound = "none";
        inbound = "public";
      };
      depends = {
        forgejo.local = true;
        nginx.local = true;
      };
      privacy = "none"; # only what was made public
    };
  };

  forgejo-runner = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    # on every box that carries it
    spread = true;
    what = "runs the fleet's own CI: builds, checks, VM tests";
    # only a fleet that develops Commonty itself builds and checks it; the
    # rest take signed releases
    when = "develops";
    # its results gate what merges into the fleet's own repository
    trust = "owner";
    traffic = {
      class = "high";
      downMbps = 100;
    };
    units = [
      "gitea-runner-*"
      "dd-runner-restart" # the runner into a new config once it holds no job
      # what its builds made, sent to the nix cache (storage/cache.nix)
      "dd-cache-upload"
      # the key what it builds is signed with in the cache, made on the box
      "dd-cache-key"
    ];
    base = {
      requires = {
        threads = 4;
        # the nix store (90G on each box, 2026-10-04) and what builds in it
        disk = {
          kind = "ssd";
          size.min = "200G";
        };
      };
      # one job's room (dd.runner.jobMemoryGiB): it takes as many jobs as the
      # box's memory holds (dd.runner.capacity), so one is what it needs to
      # run at all (node1, 2026-10-04: 11G at its peak, with its VMs); the
      # store and its own state
      uses = {
        memory = "6G";
        storage = "98G";
      };
      network = {
        outbound = [
          "github.com"
          "cache.nixos.org"
        ];
        inbound = "none";
      };
      depends = {
        forgejo.local = false; # git.<domain>, from anywhere
        garage.local = false; # the nix cache, at the storage pool's address
      };
      privacy = "none"; # the fleet's own public code
    };
    variants = {
      vm-tests.requires.kvm = true; # all but the games test
      nested-vm-tests.requires.nestedKvm = true; # the games test boots a VM in a VM
      # flake_check evaluates every check and every box's system: one
      # worker of nix-eval-jobs at about 3G at its peak (a single evaluator
      # had grown to 10.5G and took node2 out of memory beside a VM test).
      # CI sends it to a box with room for this beside the runner's own
      # use (nix/lib/ci-runners.nix), and gives a bigger box more workers
      flake-check.requires.memory.fixed = "4G";
    };
  };

  members-runner = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    # on every box that carries it
    spread = true;
    what = "runs members' own CI, sandboxed (Docker and gVisor), with no way into the fleet";
    # members' own jobs, sandboxed, with no way into the fleet
    traffic = {
      class = "high";
      downMbps = 50;
    };
    units = [ "gitea-runner-members*" ];
    base = {
      requires.threads = 4; # a job's --cpus; docker refuses more than the box has
      # one job at its limit (dd.membersRunner.jobMemoryGiB), more where the
      # box holds them; the job image
      uses = {
        memory = "4G";
        storage = "1.2G";
      };
      network = {
        outbound = "internet";
        inbound = "none";
      };
      depends.docker.local = true;
      privacy = "none"; # each job sees only itself
    };
  };
}
