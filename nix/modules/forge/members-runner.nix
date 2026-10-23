{
  config,
  pkgs,
  lib,
  utils,
  ddScript,
  ...
}:
let
  cfg = config.dd.membersRunner;
  base = config.dd.domain;
  name = "members-${config.networking.hostName}";
  # the job containers' own network; what leaves it is decided below
  # The jobs run on docker's default bridge, pinned to this subnet: what
  # leaves it is decided below. Not a user-defined network: on those docker
  # resolves names through an embedded server reached by nat rules inside
  # the container's namespace, which gVisor's network stack does not
  # carry, so nothing resolved. On the default bridge a container gets the
  # box's own nameservers and asks them directly.
  subnet = "172.30.0.0/24";
  unit = "gitea-runner-${utils.escapeSystemdPath name}";
  # on every container this runner starts, job's and services' alike (a
  # workflow's own options cannot set labels): what counts as a member's job
  label = "dd.runner=${name}";
  docker = config.virtualisation.docker.daemon.settings;
  # what of docker's config is the sandbox: the runtime and its flags, and
  # what a container's network may do
  sandboxKeys = [
    "runtimes"
    "default-runtime"
    "bip"
    "icc"
    "iptables"
    "ip6tables"
    "ip-forward"
    "ip-masq"
    "fixed-cidr"
    "default-address-pools"
    "userland-proxy"
    "no-new-privileges"
    "userns-remap"
    "seccomp-profile"
  ];
  runnerSettings = config.services.gitea-actions-runner.instances.${name}.settings;
in
{
  # one member's job, and how many of them this box holds at once
  options.dd.membersRunner.jobThreads = lib.mkOption {
    type = lib.types.ints.positive;
    default = lib.min 4 config.dd.box.threads; # docker refuses more cpus than the box has
  };
  options.dd.membersRunner.jobMemoryGiB = lib.mkOption {
    type = lib.types.ints.positive;
    default = 4;
  };
  options.dd.membersRunner.capacity = lib.mkOption {
    type = lib.types.ints.positive;
    default = lib.max 1 (
      lib.min 2 (
        lib.min (config.dd.box.threads / cfg.jobThreads) (
          config.dd.box.memoryBytes / (1024 * 1024 * 1024) / cfg.jobMemoryGiB
        )
      )
    );
    description = "members' jobs at once: what the box's threads and memory hold, at most two";
  };
  options.dd.membersRunner.drainMinutes = lib.mkOption {
    type = lib.types.ints.positive;
    default = 65;
    description = "How long a stop of the members' runner lets its jobs run on (its job limit is 1 h)";
  };
  options.dd.membersRunner.tokenFile = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    default = null;
    description = "Registration token file for the members' runner (a global token: every repo). Null: the sandbox alone, no runner registered.";
  };

  # CI for everyone's repos, on an owned box, without the job being able to
  # reach the box. A second runner, registered for every repo, runs each
  # job in a container under gVisor: the job sees a sandboxed kernel, not
  # this one. The container has no secrets, no volume from the host, no
  # docker socket, a cpu and memory cap, and a network that reaches the
  # internet and nothing of ours: the tailnet, the lan, this box. The host
  # runners (nix/modules/forge/forgejo-runner.nix) take the fleet's own repos
  # alone, which the forge enforces (dd-forge-rules in forgejo.nix): this
  # runner may stay registered for every repo only while `members` is its
  # one label. A member's workflow says `runs-on: members` and lands here.
  imports = [ ./runner-restart.nix ];

  config = {
    dd.box.plaintext = [ "members-runner (runs members' code, sandboxed)" ];

    virtualisation.docker = {
      enable = true;
      # what members' jobs leave behind goes, weekly
      autoPrune = {
        enable = true;
        flags = [
          "--all"
          "--filter"
          "until=168h"
        ];
      };
      daemon.settings = {
        runtimes.runsc.path = "${pkgs.gvisor}/bin/runsc";
        default-runtime = "runsc";
        bip = "172.30.0.1/24";
        # nothing here needs the legacy bridge to reach the host
        icc = false;
      };
    };
    # A switch never restarts docker or the members' runner: either one
    # stopping killed the jobs under way. Each restarts into its new config
    # once none of the runner's containers runs (runner-restart.nix), or
    # past the drain and a little, or at once on a new sandbox. Docker's
    # restart stops the runner first, a drain, and starts it after.
    dd.runnerRestart.units = {
      docker = {
        job = "docker";
        inherit label;
        limit = (cfg.drainMinutes + 10) * 60;
        drain = lib.optional (cfg.tokenFile != null) unit;
        sandbox = builtins.toJSON (lib.filterAttrs (k: _: lib.elem k sandboxKeys) docker);
      };
    }
    // lib.optionalAttrs (cfg.tokenFile != null) {
      ${unit} = {
        job = "docker";
        inherit label;
        limit = (cfg.drainMinutes + 10) * 60;
        sandbox = builtins.toJSON runnerSettings.container;
      };
    };

    services.gitea-actions-runner.instances.${name} = lib.mkIf (cfg.tokenFile != null) {
      enable = true;
      inherit name;
      url = "https://git.${base}";
      tokenFile = cfg.tokenFile;
      # the image a member's job runs in: the same one act uses for ubuntu
      labels = [ "members:docker://ghcr.io/catthehacker/ubuntu:act-22.04" ];
      settings = {
        runner = {
          inherit (cfg) capacity;
          timeout = "1h";
          # a stop lets the jobs under way finish first (KillMode below)
          shutdown_timeout = "${toString cfg.drainMinutes}m";
        };
        container = {
          network = "bridge";
          privileged = false;
          # nothing of the host mounted; no socket handed in
          valid_volumes = [ ];
          docker_host = "-";
          options = "--label=${label} --cpus=${toString cfg.jobThreads} --memory=${toString cfg.jobMemoryGiB}g --pids-limit=1024 --security-opt=no-new-privileges";
          force_pull = false;
        };
        cache.enabled = false;
      };
    };
    # the nixos module names the unit with systemd's escaping: the dash in
    # members-<host> becomes \x2d, and settings under the plain name went
    # to a unit that does not exist
    systemd.services.${unit} = lib.mkIf (cfg.tokenFile != null) {
      # Wants, not requires: a docker that restarts (a crash, or the
      # restart above) leaves the runner be. Under requires it stopped the
      # runner first, which waited out its job while the job waited on the
      # docker that waited on the runner: an hour of nothing.
      after = [ "docker.service" ];
      wants = [ "docker.service" ];
      # The same plain user as the host runner, not the module's dynamic
      # one: both instances live under /var/lib/gitea-runner, and a dynamic
      # user turns that into a symlink into /var/lib/private that the host
      # runner cannot work through ("mkdir: file exists" on every job). The
      # docker group is this unit's alone; host jobs do not get it.
      path = [ pkgs.curl ];
      serviceConfig = {
        DynamicUser = lib.mkForce false;
        User = lib.mkForce "forgejo-runner";
        Group = "forgejo-runner";
        SupplementaryGroups = [ "docker" ];
        # A stop is a drain: the signal goes to the runner alone, and it may
        # take the runner's job limit (1h) and a little to let them end
        KillMode = "mixed";
        TimeoutStopSec = "${toString (cfg.drainMinutes + 5)}min";
        # the forge must answer first, and at this address (runner-address.sh)
        TimeoutStartSec = "10min";
        ExecStartPre = lib.mkBefore (
          # its token, made by the forge's box and sealed to this one
          # (runner-keys.nix), where the host runner's module is here too
          lib.optional (
            config.dd ? runner && config.dd.runner ? fetchToken
          ) "+${config.dd.runner.fetchToken "members"}"
          ++ [
            (pkgs.writeShellScript "runner-address" (
              ddScript ./runner-address.sh {
                NAME = name;
                URL = "https://git.${base}";
              }
            ))
          ]
        );
      };
    };

    # What a job may reach: the internet. Docker's DOCKER-USER chain runs
    # before its own forwarding rules; established replies come back, the
    # rest of what is ours is dropped before it leaves the bridge.
    networking.firewall.extraCommands = ''
      iptables -N DOCKER-USER 2>/dev/null || true
      iptables -F DOCKER-USER
      iptables -A DOCKER-USER -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN
      for dst in 100.64.0.0/10 10.0.0.0/8 192.168.0.0/16 172.16.0.0/12 169.254.0.0/16 127.0.0.0/8; do
        iptables -A DOCKER-USER -s ${subnet} -d $dst -j DROP
      done
      iptables -A DOCKER-USER -j RETURN
      # and the box itself, on any of its addresses
      iptables -I nixos-fw 1 -s ${subnet} -j DROP
    '';
    networking.firewall.extraStopCommands = ''
      iptables -F DOCKER-USER 2>/dev/null || true
    '';
  };
}
