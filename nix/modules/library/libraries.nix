# The members' encrypted libraries (crate library): one bucket, a prefix per
# library, and the verifier's gate as the only way in. The bucket is in this
# box's own store, and the box holds ciphertext and the store's key for the
# bucket; a member's device never sees that key, it asks the gate, which
# moves the bytes of one object at a time.
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
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        assertions = [
          {
            # the bucket is in the store of the box that writes it alone
            # (box/host.nix): the old shared pool is gone
            assertion = own;
            message = "the libraries are in the store of the box that writes them: dd.store.buckets.libraries.writer must be this box";
          }
        ];
        # the gate's keys are the store's, one for each bucket, made here; the
        # gate and its transcoder reach the store on loopback
        systemd.services.dd-library-keys = {
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
          s3 = "http://127.0.0.1:${toString store.port}";
          bucket = "libraries";
          keyFile = "${keys}/env";
        };
      }
      (lib.optionalAttrs (options.dd ? transcode) {
        dd.transcode.source = "http://127.0.0.1:${toString store.port}/libraries/";
      })
    ]
  );
}
