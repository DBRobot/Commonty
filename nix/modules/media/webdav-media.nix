{
  config,
  options,
  lib,
  ...
}:
let
  base = config.dd.domain;
  host = "files.${base}";
  images = "/srv/images";
in
{
  # where the archives were, in the box's bulk pool: kept, untouched, until
  # its owner lets it go (they are in the bucket now)
  config = lib.mkMerge [
    (lib.optionalAttrs (options.dd ? zfs && options.dd ? box) {
      dd.zfs.datasets."${config.dd.box.bulkPool}/enc/images" = {
        mountpoint = images;
        recordsize = "1M";
        compression = "lz4";
        "com.sun:auto-snapshot" = "false";
      };
    })
    {
      # A member's files are their library: named
      # and encrypted on their own machine, stored as ciphertext under
      # /_dd/dav (modules/gate/verify.nix, servers/library/src/dav.rs), opened in
      # the browser at /_dd/files or mounted with `dd media`. This host keeps
      # /images/ too: `dd image`'s archives of old computers, restic's
      # format, encrypted on the device.
      services.nginx.virtualHosts.${host} = {
        useACMEHost = base;
        forceSSL = true;

        # the door a person lands on: their library, not a folder on this box
        locations."/".extraConfig = ''
          return 302 https://files.${base}/_dd/files;
        '';
        # The disk images are in the bucket now, beside the libraries: the
        # gate serves them (servers/gate/src/library.rs, images) and decides
        # who, so nothing here asks first. /srv/images is what they were
        # copied from, kept until its owner lets it go.
        locations."/images/" = {
          proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/images/";
          extraConfig = ''
            client_max_body_size 0; # disk images
            proxy_request_buffering off;
            proxy_buffering off;
            client_body_timeout 600s;
            proxy_read_timeout 600s;
            proxy_send_timeout 600s;
          '';
        };

      };

    }
  ];
}
