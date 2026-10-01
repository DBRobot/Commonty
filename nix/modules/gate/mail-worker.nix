# The mail Worker (client/mail): members' addresses live there, at
# Cloudflare, and nowhere on a box. This box carries the Worker's files in
# its own closure, so the boxes that vouch for a release vouch for them too,
# and checks hourly that what Cloudflare runs is exactly that. The check's
# token reads scripts and nothing else; without one, there is no check.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.dd.mailWorker;
  bundle = self.packages.${pkgs.stdenv.hostPlatform.system}.mail-worker;
in
{
  options.dd.mailWorker = {
    enable = lib.mkEnableOption "the mail Worker's files here, and the check that Cloudflare runs them";
    account = lib.mkOption {
      type = lib.types.str;
      default = "";
      description = "the Cloudflare account the Worker is in (fleet/mail.json, written by `dd mail setup`)";
    };
    script = lib.mkOption {
      type = lib.types.str;
      default = "commonty-mail";
      description = "the Worker's name at Cloudflare";
    };
    readTokenFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "env file with CF_WORKERS_READ_TOKEN: Workers Scripts Read, nothing more (sops: cloudflare-workers-read)";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.etc."dd/mail-worker".source = bundle;

    systemd.services.dd-mail-worker-check = lib.mkIf (cfg.readTokenFile != null && cfg.account != "") {
      description = "Check the mail Worker at Cloudflare is this release's";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      startAt = "hourly";
      environment = {
        ACCOUNT = cfg.account;
        SCRIPT = cfg.script;
        BUNDLE = "${bundle}";
        FACTS = "/var/lib/dd-facts";
      };
      serviceConfig = {
        Type = "oneshot";
        User = "node-exporter";
        Group = "node-exporter";
        EnvironmentFile = cfg.readTokenFile;
        ExecStart = "${pkgs.python3}/bin/python3 ${./mail-worker-check.py}";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ReadWritePaths = [ "/var/lib/dd-facts" ];
      };
    };
  };
}
