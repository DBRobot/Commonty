{
  config,
  ...
}:
let
  base = config.dd.domain;
  host = "files.${base}";
in
{
  config = {
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
      # The disk images are in the bucket, beside the libraries: the gate
      # serves them (servers/gate/src/library.rs, images) and decides who,
      # so nothing here asks first.
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
      # The same images under /_dd/, which is where the box installer's
      # restic sends them. Without its own location they fell into the
      # gate's /_dd/ and its 10m body limit, which refuses most of restic's
      # ~16 MiB packs: the backup retried one pack for hours.
      locations."/_dd/images/" = {
        proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/images/";
        extraConfig = ''
          client_max_body_size 0; # disk images
          proxy_request_buffering off;
          proxy_buffering off;
          client_body_timeout 600s;
          proxy_read_timeout 600s;
          proxy_send_timeout 600s;
          proxy_set_header X-Original-URI $request_uri;
          limit_req zone=dd_user burst=300 nodelay;
          limit_req zone=dd_visitor burst=500 nodelay;
          limit_req_status 429;
        '';
      };

    };

  };
}
