{
  config,
  lib,
  pkgs,
  ...
}:
let
  port = 8888;
  run = "/run/dd-search";
in
{
  # Chat's web search. The model has no network at all (llama-cpp.nix);
  # the page asks the gate, the gate asks SearXNG here on loopback, and
  # only titles, links and a line of each go back to be put in front of the
  # model. SearXNG goes out to the search engines and nowhere else: nothing
  # reaches it from outside this box, and it reaches nothing inside the
  # house or the tailnet.
  dd.box.plaintext = [ "searxng (web search queries)" ];

  services.searx = {
    enable = true;
    environmentFile = "${run}/env";
    settings = {
      use_default_settings = true;
      server = {
        bind_address = "127.0.0.1";
        inherit port;
        secret_key = "$SEARX_SECRET_KEY";
        # one caller, the gate, which already checked who is asking
        limiter = false;
        public_instance = false;
        image_proxy = false;
      };
      search = {
        formats = [
          "html"
          "json"
        ];
        safe_search = 1;
      };
      outgoing.request_timeout = 5.0;
    };
  };

  # the key SearXNG signs its own cookies with, made at boot like the
  # model server's; nothing outside this box ever holds it
  systemd.services.dd-search-key = {
    description = "the key SearXNG signs with";
    wantedBy = [ "multi-user.target" ];
    before = [
      "searx-init.service"
      "searx.service"
    ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      UMask = "0077";
    };
    script = ''
      install -d -m 0700 ${run}
      [ -s ${run}/env ] || printf 'SEARX_SECRET_KEY=%s\n' "$(${pkgs.openssl}/bin/openssl rand -hex 32)" > ${run}/env
    '';
  };
  systemd.services.searx-init = {
    after = [ "dd-search-key.service" ];
    requires = [ "dd-search-key.service" ];
  };

  # Out to the engines only. A result page can point anywhere, and a
  # search engine that follows links for it could be asked to fetch the
  # router or another box; those addresses are closed to it.
  systemd.services.searx = {
    after = [ "dd-search-key.service" ];
    requires = [ "dd-search-key.service" ];
    serviceConfig = {
      IPAddressDeny = [
        "10.0.0.0/8"
        "172.16.0.0/12"
        "192.168.0.0/16"
        "100.64.0.0/10"
        "169.254.0.0/16"
        "fc00::/7"
        "fe80::/10"
      ];
      NoNewPrivileges = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
    };
  };

  # the gate on this box answers Chat's searches through it
  systemd.services.dd-verify = lib.mkIf config.dd.verify.enable {
    environment.VERIFY_SEARCH = "http://127.0.0.1:${toString port}";
  };
}
