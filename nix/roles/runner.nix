{
  config,
  lib,
  ...
}:
{
  # A CI runner for the forge, registered under this box's name with the
  # nix label. Stateless: any box can run one, jobs land on whichever is
  # free, and CI keeps going while another box is being reinstalled. What
  # it builds goes to the nix-cache bucket, signed with a key this box made, so
  # the next run and the next reinstall find it there.
  imports = [
    ./_sops.nix
    ../modules/forge/forgejo-runner.nix
    ../modules/forge/members-runner.nix
    ../modules/storage/cache.nix
  ];

  config = lib.mkMerge [
    {
      # the members' runner registers for every repo; its global token is
      # made by the forge's box and sealed to this one, like the host
      # runner's (modules/forge/runner-keys.nix)
      dd.membersRunner.tokenFile = "/var/lib/dd-runner-token/members.env";
      # the home controller's system is aarch64 (docs/dev/home-controller.md):
      # its own programs are cross-built, everything else comes from
      # cache.nixos.org, and the few steps left run here under qemu
      boot.binfmt.emulatedSystems = [ "aarch64-linux" ];
      # what it uploads is signed with a key it made itself (storage/cache.nix);
      # nix's key to the cache is this box's key to the store that keeps it
      # (storage/cache.nix)
    }
  ];
}
