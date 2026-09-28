# The compute side of the encrypted libraries: a member's device asks this
# box to play a file its own player cannot. The device brings the chunk
# urls and the file key sealed to this box's key; the box decrypts in
# memory, transcodes, streams, and wipes the session. Plaintext exists
# here only in RAM while working, which is the fleet's rule for compute.
{
  config,
  pkgs,
  lib,
  self,
  ...
}:
let
  cfg = config.dd.transcode;
  # the box's GPU, if its hardware file names one: encoding goes there, and
  # the service gets that one device; a picture it will not take falls back
  # to the cores
  vaapi = config.dd.box.vaapi;
  base = config.dd.domain;
  port = 4190;
in
{
  options.dd.transcode.enable = lib.mkEnableOption "transcoding one file at a time for the libraries' players";
  options.dd.transcode.source = lib.mkOption {
    type = lib.types.str;
    default = "https://s3.${base}/libraries/";
    description = "the only url prefix a session may fetch from: the libraries bucket, as the gate presigns it. A member sends the url, so anything else is this box fetching wherever a member points it.";
  };

  config = lib.mkIf cfg.enable {
    # a label, for honesty: prompts to llama.cpp are the same shape
    dd.box.plaintext = [ "transcode (a file in memory while it plays)" ];

    systemd.services.dd-transcode = {
      description = "Transcode one encrypted file for one viewer, in memory";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      environment = {
        TRANSCODE_BIND = "127.0.0.1:${toString port}";
        TRANSCODE_PLAIN_BIND = "127.0.0.1:${toString (port + 1)}";
        TRANSCODE_DIR = "/run/dd-transcode";
        TRANSCODE_FFMPEG = "${pkgs.ffmpeg-headless}/bin/ffmpeg";
        TRANSCODE_SOURCE = cfg.source;
      }
      // lib.optionalAttrs (vaapi != null) {
        TRANSCODE_VAAPI = vaapi;
        # the driver comes from hardware.graphics, in the box's hardware file
        LIBVA_DRIVERS_PATH = "/run/opengl-driver/lib/dri";
      };
      serviceConfig = {
        Type = "simple";
        DynamicUser = true;
        # the sessions live in tmpfs and go with the unit
        RuntimeDirectory = "dd-transcode";
        RuntimeDirectoryMode = "0700";
        ExecStart = "${self.packages.${pkgs.stdenv.hostPlatform.system}.transcode}/bin/dd-transcode";
        Restart = "on-failure";
        NoNewPrivileges = true;
        # ffmpeg here is a parser fed a member's own media, and a parser
        # fed hostile bytes is where a crash becomes something else. A core
        # dump of this process would be decoded frames on disk.
        LimitCORE = 0;
        SystemCallFilter = [
          "@system-service"
          "~@obsolete"
          "~@privileged"
          "~@resources"
        ];
        SystemCallArchitectures = "native";
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        RestrictNamespaces = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        MemoryMax = "4G";
      }
      // lib.optionalAttrs (vaapi != null) {
        # the render node alone: no display, no other device
        DeviceAllow = [ "${vaapi} rw" ];
        SupplementaryGroups = [ "render" ];
      };
    };

    # reached through the gate's host, by a signed-in member only; the plain
    # listener (port + 1) is for ffmpeg and is proxied by nothing
    services.nginx.virtualHosts."files.${base}".locations =
      let
        # behind the gate: starting a session, and the box's key to seal to
        gated = {
          extraConfig = ''
            auth_request /_dd/verify;
            # the session it makes is its own credential; it never replays
            # the caller's
            proxy_set_header Authorization "";
            proxy_set_header Cookie $dd_cookie_stripped;
            client_max_body_size 8m;
            proxy_read_timeout 120s;
          '';
        };
      in
      {
        "/_dd/transcode/" = gated // {
          proxyPass = "http://127.0.0.1:${toString port}/";
        };

        # Starting a session, exactly. Without this, nginx answers a request
        # for the path below minus its slash with a redirect to it, as it
        # does for any location ending in a slash - a POST turned into a GET
        # of an empty session, and no film ever started from a page.
        "= /_dd/transcode/session" = gated // {
          proxyPass = "http://127.0.0.1:${toString port}/session";
        };

        # The playlist and its segments, on the session id alone. A player
        # element fetches these itself and cannot be made to carry a token
        # or a cookie, so the id is the credential: 128 bits from
        # /dev/urandom, minted only for a member who asked for this one
        # file, good only while the session lives. That is what a presigned
        # url is, and the gate already hands those out (box/library/src/dav.rs). Starting a
        # session is still behind the gate above; this is only watching one
        # that somebody already started.
        "/_dd/transcode/session/" = {
          proxyPass = "http://127.0.0.1:${toString port}/session/";
          extraConfig = ''
            proxy_read_timeout 120s;
            # a segment is written as it is made: no buffering in the way
            proxy_buffering off;
          '';
        };
      };
  };
}
