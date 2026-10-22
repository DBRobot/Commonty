# A release that changes the runner, switched while a CI job runs. Release
# 331 changed the runner's unit; the switch stopped it, and the next pull
# request's run (786), in flight, died with "context canceled". Here a job
# waits on the test while the box switches to a runner with a new config
# and a restarted gate; the job must finish and pass, and the runner must
# come up on the new config once it holds no job. A stop that does come
# (one the restart unit asks for in the instant a job is taken) drains.
# The members' runner the same: a member's job, in its container, lives
# through a switch that changes the members' runner and docker's config,
# and both restart into theirs once no job's container runs. But not for
# ever: a member's job that outlasts the limit is drained and docker
# restarts all the same, and a change to the sandbox itself restarts docker
# on the next pass, cancelling the job on the old one.
{ pkgs, self, ... }:
let
  # the release above
  next =
    { lib, ... }:
    {
      dd.runner.capacity = lib.mkForce 2;
      dd.membersRunner.capacity = lib.mkForce 2;
      virtualisation.docker.daemon.settings.labels = [ "release=next" ];
      systemd.services.dd-verify.environment.DD_TEST_RELEASE = "next";
    };
  # a later one that changes docker's config and nothing of the sandbox
  later =
    { lib, ... }:
    {
      imports = [ next ];
      virtualisation.docker.daemon.settings.labels = lib.mkForce [ "release=later" ];
    };
  # and one that changes the sandbox: gVisor's flags
  sandbox = {
    imports = [ later ];
    virtualisation.docker.daemon.settings.runtimes.runsc.runtimeArgs = [ "--platform=systrap" ];
  };
  # what a member's job runs in, under the name the runner's label gives,
  # so nothing is pulled
  jobImage = pkgs.dockerTools.buildImage {
    name = "ghcr.io/catthehacker/ubuntu";
    tag = "act-22.04";
    copyToRoot = pkgs.buildEnv {
      name = "job-root";
      paths = [ pkgs.busybox ];
      pathsToLink = [ "/bin" ];
    };
    config.Cmd = [ "/bin/sh" ];
  };
  cert = pkgs.runCommand "test-cert" { nativeBuildInputs = [ pkgs.openssl ]; } ''
    mkdir $out
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3650 \
      -subj '/CN=test.invalid' -addext 'subjectAltName=DNS:*.test.invalid' \
      -keyout $out/key.pem -out $out/cert.pem
  '';
in
{
  name = "runner-restart";
  helpers = true;
  node.specialArgs = { inherit self; };
  nodes.box =
    { config, lib, ... }:
    {
      imports = [
        ./box.nix
        ../modules/forge/forgejo.nix
        ../modules/forge/forgejo-runner.nix
        ../modules/forge/members-runner.nix
        # what the runner module reads of sops: its token, here a file the
        # test writes once the forge has minted one
        { options.sops = lib.mkOption { type = lib.types.anything; }; }
      ];
      sops.placeholder.forgejo-runner-token = "unused";
      sops.templates."forgejo-runner.env".path = "/run/runner.env";
      # started by the test once its token is there, not at boot
      systemd.services.gitea-runner-box.wantedBy = lib.mkForce [ ];
      systemd.services."gitea-runner-members\\x2dbox".wantedBy = lib.mkForce [ ];
      dd.membersRunner = {
        tokenFile = "/run/runner.env";
        capacity = 1;
        jobThreads = 1;
        jobMemoryGiB = 1;
        # a stop of the members' runner drains for a minute, and a change
        # waits four for no job, not an hour and more
        drainMinutes = 1;
      };
      dd.runnerRestart.units.docker.limit = lib.mkForce 240;
      dd.runnerRestart.units."gitea-runner-members\\x2dbox".limit = lib.mkForce 240;
      system.switch.enable = true;
      dd.forgejo.admin = "tester";
      dd.verify.role = lib.mkForce "full";
      dd.runner.capacity = 1;
      systemd.services.zfs-datasets = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
        };
        script = "mkdir -p /vault";
      };
      # the runner reaches the forge by its name, as on a box
      networking.hosts."127.0.0.1" = [ "git.test.invalid" ];
      security.pki.certificateFiles = [ "${cert}/cert.pem" ];
      services.nginx.enable = true;
      services.nginx.recommendedProxySettings = true;
      services.nginx.virtualHosts =
        lib.genAttrs (map (h: "${h}.test.invalid") (config.dd.verify.hosts ++ [ "code" ]))
          (_: {
            useACMEHost = lib.mkForce null;
            sslCertificate = "${cert}/cert.pem";
            sslCertificateKey = "${cert}/key.pem";
          });
      # where a job says it started and the test says it may end
      # and the box's facts, which say how long a change has waited
      systemd.tmpfiles.rules = [
        "d /run/job-gate 0777 root root -"
        "d /var/lib/dd-facts 0755 root root -"
      ];
      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
      ];
      # the release: a runner with a new config, and a gate that the switch
      # stops and starts again, as 331's did
      specialisation.next.configuration = next;
      specialisation.later.configuration = later;
      specialisation.sandbox.configuration = sandbox;
      # docker starts when the members' runner does, after the forge is up,
      # not beside postgres and the forge at boot
      virtualisation.docker.enableOnBoot = false;
      virtualisation.cores = 2;
      virtualisation.memorySize = 4096;
      virtualisation.diskSize = 6144;
    };
  scriptEnv = {
    admin = "tester";
    sock = "/run/forgejo/forgejo.sock";
    runner = "gitea-runner-box.service";
    members = "gitea-runner-members\\x2dbox.service";
    inherit jobImage;
  };
}
