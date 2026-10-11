# The members' encrypted libraries (crate library): one bucket, a prefix per
# library, and the verifier's gate as the only way in. The box holds
# ciphertext and one key of its own for the bucket; a member's device never
# sees that key, it asks the gate, which moves the bytes of one object at a
# time.
{
  config,
  lib,
  ...
}:
let
  cfg = config.dd.libraries;
in
{
  options.dd.libraries = {
    enable = lib.mkEnableOption "the members' encrypted libraries on this box";
    keyFile = lib.mkOption {
      type = lib.types.path;
      description = "env file with LIBRARY_ID and LIBRARY_SECRET: the gate's garage key, imported at setup";
    };
  };

  config = lib.mkIf cfg.enable {
    dd.garage.setupEnvFiles = [ cfg.keyFile ];
    dd.garage.buckets.libraries = {
      key = {
        name = "dd-library";
        envPrefix = "LIBRARY";
      };
      allow = [
        "read"
        "write"
      ];
      # no CORS: no page reaches the bucket. Devices go through the gate,
      # which streams the bytes, and the one presigned url handed out goes
      # to the transcoder, which fetches it server to server
    };
    # what a visitor sees on the Files page: a library of its own, empty,
    # read-only. The key is in the open because the demo cannot keep one.
    dd.verify.demoLibrary = {
      id = "e14dbb2a30e3096a5a9bc42ace4599b1";
      key = "qeeZQepVeE9uPrWoujkziNKB93rootOf1KH5MZeKqYM=";
    };

    dd.verify.library = {
      s3 = "https://s3.${config.dd.domain}";
      bucket = "libraries";
      keyFile = cfg.keyFile;
    };
  };
}
