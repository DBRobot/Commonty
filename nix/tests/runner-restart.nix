# A release that changes the runner, switched while a CI job runs. Release
# 331 changed the runner's unit; the switch stopped it, and the next pull
# request's run (786), in flight, died with "context canceled". Here a job
# waits on the test while the box switches to a runner with a new config
# and a restarted gate; the job must finish and pass, and the runner must
# come up on the new config once it holds no job. A stop that does come
# (one the restart unit asks for in the instant a job is taken) drains.
{ pkgs, self, ... }:
let
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
        # what the runner module reads of sops: its token, here a file the
        # test writes once the forge has minted one
        { options.sops = lib.mkOption { type = lib.types.anything; }; }
      ];
      sops.placeholder.forgejo-runner-token = "unused";
      sops.templates."forgejo-runner.env".path = "/run/runner.env";
      # started by the test once its token is there, not at boot
      systemd.services.gitea-runner-box.wantedBy = lib.mkForce [ ];
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
      systemd.tmpfiles.rules = [ "d /run/job-gate 0777 root root -" ];
      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
      ];
      # the release: a runner with a new config, and a gate that the switch
      # stops and starts again, as 331's did
      specialisation.next.configuration = {
        dd.runner.capacity = lib.mkForce 2;
        systemd.services.dd-verify.environment.DD_TEST_RELEASE = "next";
      };
      virtualisation.memorySize = 3072;
      virtualisation.diskSize = 4096;
    };
  scriptEnv = {
    admin = "tester";
    sock = "/run/forgejo/forgejo.sock";
    runner = "gitea-runner-box.service";
  };
}
