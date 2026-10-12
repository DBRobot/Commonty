{ config, lib, ... }:
let
  policy = lib.importJSON ../../fleet/policy.json;
  boxes = lib.importJSON ../../fleet/boxes.json;
  report = n: ../../fleet/boxes + "/${n}/hardware.json";
  # each box that runs CI, and what its own registration tokens are sealed to
  runners = lib.mapAttrs (n: _: (lib.importJSON (report n)).secretsKey) (
    lib.filterAttrs (
      n: b:
      builtins.elem "runner" (b.roles or [ ])
      && builtins.pathExists (report n)
      && (lib.importJSON (report n)) ? secretsKey
    ) boxes
  );
in
{
  # The forge and the mirror that keeps GitHub's copy current. Its runner
  # is the runner role: any box can run one, the forge runs on one.
  imports = [
    ./_sops.nix
    ../modules/forge/forgejo.nix
    ../modules/forge/forgejo-mirror.nix
  ];
  dd.home.services = [
    {
      name = "Code";
      blurb = "Git repositories, with their history and reviews.";
      url = "https://git.${config.dd.domain}/";
      # the forge is told nobody is there (modules/forgejo.nix): public
      # repos, no account
      demo = "full";
      # the demo has no repositories of its own: it lands on the fleet's
      # own, which is public, on the forge's demo door
      demoUrl = "https://demo-git.${config.dd.domain}/${config.dd.repo}";
      icon = "code";
      color = "#4a5a8a";
      rank = 50;
    }
  ];
  # No person administers the forge: the box's own identity does its jobs
  # (dd.forgejo.admin). The mirrors are the fleet's choice (fleet/policy.json)
  dd.backup.paths = [ config.dd.forgejo.stateDir ]; # repositories, lfs, custom config; its db is in the postgres dumps
  # the runners' tokens are made here, by the box, never by a person
  dd.forgejo.runnerBoxes = runners;
  dd.forgejo.mirrors = map (m: m // { repo = m.repo or config.dd.repo; }) (
    policy.forgeMirrors or [ ]
  );
}
