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
  options.dd.runner.capacity = lib.mkOption {
    type = lib.types.int;
    default = 2;
    description = "Jobs this box runs at once. A vm test is a few vms of 1-2G each; the 14G box takes two, a 62G box more.";
  };

  config = {
    # reads plaintext: only on a box whose owner is trusted with it (modules/box.nix)
    dd.box.plaintext = [ "forgejo-runner (runs contributed code)" ];
    # One runner, on this box, in host mode: a job runs as the runner user with
    # nix on the path and builds straight into this box's store. So the closure
    # a green check just built is already here, and a deploy is a switch.
    #
    # Host mode means a job is code running on node1 as the runner's dynamic
    # user. Fine while every author is the admin; the day strangers can open
    # pull requests here this becomes a container runner or a separate box.
    services.gitea-actions-runner = {
      package = pkgs.forgejo-runner;
      instances.${name} = {
        enable = true;
        inherit name;
        url = "https://git.${base}";
        # a registration token, minted once with `forgejo actions
        # generate-runner-token` and kept in sops; the runner registers on
        # first start and keeps its own credential in its state dir after
        tokenFile = config.sops.templates."forgejo-runner.env".path;
        # nix: the job runs on a box with nix. gating: what this box builds and
        # reports is what the fleet installs and what merges main; that is
        # only ever a box the release signer owns (roles/runner.nix). A
        # stranger's box, when there is one, gets a runner without the second
        # label, a read-only cache key, and jobs whose verdicts are advisory.
        # and the box's own name, for a job that must run on this box and
        # no other: each box building every box's system (nix_checks.yml),
        # so that what it later vouches for it has built itself
        labels = [
          "nix:host"
          "gating:host"
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
