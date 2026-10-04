{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.cache;
  # the storage pool at this box's address for it: its own garage, or a
  # storage box's over the tailnet (dd.box.s3)
  url = "s3://nix-cache?endpoint=${lib.removePrefix "http://" config.dd.box.s3}&scheme=http&region=us-east-1";
  # Only the laptop's. A box's nix installs whatever it substitutes, so a
  # key in this list is a machine allowed to decide what every box that
  # reads the cache runs - and node2 is meant to sit in someone else's
  # house. Boxes still upload what they build, signed with their own key;
  # nobody has to believe them for the cache to be useful, because the
  # agent checks the release's hashes and the whole closure under them.
  all = lib.filter (k: k != "") (
    lib.splitString "\n" (lib.fileContents ../../../fleet/cache-keys.pub)
  );
  keys = lib.filter (k: lib.hasPrefix "dd-cache-laptop-" k) all;
in
{
  # The nix cache is the nix-cache bucket in the cluster. A box with this
  # module substitutes from it and, through a post-build hook, uploads
  # whatever it builds, signed with its own key. The laptop signs what it
  # publishes with its key. fleet/cache-keys.pub lists every key a box's
  # nix accepts; the agent does not rely on them, it checks nar hashes from
  # the signed release and every path under it, so a poisoned upload can at
  # most mislead CI on the box that reads it, never a release.
  options.dd.cache = {
    signingKeyFile = lib.mkOption {
      type = lib.types.str;
      description = "This box's nix signing key, root-only.";
    };
    credentialsFile = lib.mkOption {
      type = lib.types.str;
      default = "/root/.aws/credentials";
      description = "AWS credentials file with this box's cache key, for nix as root.";
    };
  };

  config = {
    nix.settings = {
      substituters = [ url ];
      trusted-public-keys = keys;
      post-build-hook = pkgs.writeShellScript "upload-to-cache" ''
        set -eu
        export HOME=/root
        export AWS_SHARED_CREDENTIALS_FILE=${cfg.credentialsFile}
        # nix waits on this hook, so a VM test's 1.5 GB disk image held CI for
        # ten minutes; nothing big is worth the wait, and test images are
        # rebuilt in seconds
        paths=()
        for p in $OUT_PATHS; do
          case $p in *-nixos-disk-image) continue ;; esac
          [ "$(${pkgs.coreutils}/bin/du -s --block-size=1M "$p" | ${pkgs.coreutils}/bin/cut -f1)" -gt 256 ] && continue
          paths+=("$p")
        done
        [ ''${#paths[@]} -gt 0 ] || exit 0
        # never fail a build over the cache; it is a copy, not the result
        ${config.nix.package}/bin/nix --extra-experimental-features nix-command \
          copy --to '${url}&secret-key=${cfg.signingKeyFile}' "''${paths[@]}" \
          || echo "cache upload failed for ''${paths[*]}" >&2
      '';
    };
    # the daemon substitutes as root; its credentials live at the usual place
    systemd.services.nix-daemon.environment.AWS_SHARED_CREDENTIALS_FILE = cfg.credentialsFile;
    systemd.services.nix-daemon.environment.HOME = "/root";
  };
}
