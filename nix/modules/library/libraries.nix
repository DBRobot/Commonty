# The members' encrypted libraries (crate library): one bucket, a prefix per
# library, and the verifier's gate as the only way in. The box holds
# ciphertext and one key of its own for the bucket; a member's device never
# sees that key, it asks the gate, which moves the bytes of one object at a
# time.
{
  config,
  options,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.libraries;
  store = config.dd.store;
  # this box writes the libraries' bucket in its own store (storage/store.nix)
  own =
    options.dd ? store
    && store.enable
    && (store.buckets.libraries.writer or null) == config.networking.hostName;
  keys = "/var/lib/dd-library-keys";
in
{
  options.dd.libraries = {
    enable = lib.mkEnableOption "the members' encrypted libraries on this box";
    keyFile = lib.mkOption {
      type = lib.types.path;
      description = "env file with LIBRARY_ID and LIBRARY_SECRET: the gate's key to the old pool, imported there at setup, and what the move into the store reads it with";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        # in this box's own store: the gate's keys are the store's, one for each
        # bucket, made here; the gate and its transcoder reach it on loopback
        systemd.services.dd-library-keys = lib.mkIf own {
          description = "The libraries' and the mail's keys to this box's store, for the gate";
          wantedBy = [ "dd-verify.service" ];
          before = [ "dd-verify.service" ];
          after = [ "dd-store-setup.service" ];
          requires = [ "dd-store-setup.service" ];
          path = [ pkgs.coreutils ];
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
          };
          script = ''
            set -euo pipefail
            umask 077
            install -d -m 0700 ${keys}
            {
              ( . /var/lib/dd-store/keys/libraries.env
                echo "LIBRARY_ID=$AWS_ACCESS_KEY_ID"
                echo "LIBRARY_SECRET=$AWS_SECRET_ACCESS_KEY" )
              ( . /var/lib/dd-store/keys/mail.env
                echo "MAIL_ID=$AWS_ACCESS_KEY_ID"
                echo "MAIL_SECRET=$AWS_SECRET_ACCESS_KEY" )
            } > ${keys}/env.new
            chmod 0400 ${keys}/env.new
            mv ${keys}/env.new ${keys}/env
          '';
        };
        # what a visitor sees on the Files page: a library of its own, empty,
        # read-only. The key is in the open because the demo cannot keep one.
        dd.verify.demoLibrary = {
          id = "e14dbb2a30e3096a5a9bc42ace4599b1";
          key = "qeeZQepVeE9uPrWoujkziNKB93rootOf1KH5MZeKqYM=";
        };

        dd.verify.library = {
          s3 = if own then "http://127.0.0.1:${toString store.port}" else "https://s3.${config.dd.domain}";
          bucket = "libraries";
          keyFile = if own then "${keys}/env" else cfg.keyFile;
        };
      }
      (lib.optionalAttrs (options.dd ? transcode) {
        dd.transcode.source = lib.mkIf own "http://127.0.0.1:${toString store.port}/libraries/";
      })
      # the old pool, until it goes: the bucket there, and the copy into the
      # store, which reads with this key
      # a box with no store (the leaving test's) has no move to make
      (lib.optionalAttrs (options.dd ? garage && options.dd ? store) {
        dd.store.migrate = lib.mkIf own (
          lib.genAttrs [ "libraries" "mail" ] (_: {
            envFile = cfg.keyFile;
            id = "LIBRARY_ID";
            secret = "LIBRARY_SECRET";
            # the gate writes the store: once more, then no more
            once = true;
          })
        );
      })
      (lib.optionalAttrs (options.dd ? garage) {
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
      })
    ]
  );
}
