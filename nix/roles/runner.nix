{
  config,
  lib,
  options,
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
      # the members' runner registers for every repo; its token is global
      sops.secrets.forgejo-members-runner-token = { };
      sops.templates."forgejo-members-runner.env".content = ''
        TOKEN=${config.sops.placeholder.forgejo-members-runner-token}
      '';
      dd.membersRunner.tokenFile = config.sops.templates."forgejo-members-runner.env".path;
      # the home controller's system is aarch64 (docs/dev/home-controller.md):
      # its own programs are cross-built, everything else comes from
      # cache.nixos.org, and the few steps left run here under qemu
      boot.binfmt.emulatedSystems = [ "aarch64-linux" ];
      # what it uploads is signed with a key it made itself (storage/cache.nix);
      # nix's key to the cache is this box's key to the store that keeps it
      # (storage/cache.nix); the sops one is the old pool's, until it goes
      sops.secrets.cache-key-id = { };
      sops.secrets.cache-key-secret = { };
    }
    (lib.optionalAttrs (options.dd ? garage) {
      # a runner writes the cache it reads: where this box runs garage, its key
      # gets write as well as read
      dd.garage.buckets.nix-cache.allow = [
        "read"
        "write"
      ];
    })
  ];
}
