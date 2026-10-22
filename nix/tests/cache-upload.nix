# What a box builds reaches the nix cache (storage/cache.nix). On
# 2026-10-22 every build's queue file started the uploader again, systemd's
# start limit refused it for good, and nothing more was sent: release 352
# found the home controller's system missing from the cache. Here builds in
# rounds start it over and over, a closure too big for the base timeout
# goes, and uploads to a cache that refuses them wait and go once it is back.
{ pkgs, ... }:
{
  name = "cache-upload";
  nodes.box =
    { lib, ... }:
    {
      imports = [
        ../modules/storage/cache.nix
        { options.dd.box.s3 = lib.mkOption { default = "http://127.0.0.1:1"; }; }
      ];
      dd.cache = {
        url = "file:///var/lib/test-cache";
        signingKeyFile = "/var/lib/test-cache.sec";
        # far below what the 300 MiB path takes to compress and write; only
        # the per-MiB part of the timeout lets it through
        uploadTimeout = 1;
        retryDelay = 2;
      };
      # builds here are the test's own; nothing to substitute
      nix.settings.substituters = lib.mkForce [ ];
      environment.systemPackages = [ pkgs.busybox ];
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 4096;
      virtualisation.writableStoreUseTmpfs = false;
    };
  scriptEnv = {
    sh = "${pkgs.busybox}/bin/sh";
    cache = "file:///var/lib/test-cache";
  };
}
