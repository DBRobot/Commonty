# A box takes photos over (failover: box/box.nix): a runs postgres and
# museum at epoch 1, with keys it made itself, and a member's account is
# made; the archive and the photos bucket go to b's store as copies. b takes
# epoch 2: postgres restored from its own store's copy, museum with keys b
# made itself, which a never had - and the member's address finds the same
# account there. The flags dd-control would raise are raised by hand.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  # test keys only: what each box's TPM-held secrets key is on a real one
  ages = {
    a = {
      identity = "AGE-SECRET-KEY-1RYQV6YMT4DPUURXLGDHLEP6P4QLM4NYQ4Z4E594SKSVYTXHZTF6QAJ94TJ";
      recipient = "age1gc25m409smgcklvytkksgew4ycjky4xtwvarsynwjxa9eg60jgpqfrlgp8";
    };
    b = {
      identity = "AGE-SECRET-KEY-18PLY0LS6RNEJLKPAYQRU6FD35UMDPS7WLXV94ASZ4G286NWRKSJQQPJL2V";
      recipient = "age1qg66feuknweacfxtff0mfejky0xzuk8d23zssz763lpmptxxquessgd8hd";
    };
  };
  other = name: if name == "a" then "b" else "a";
  handing = import ../lib/not-handing.nix { inherit pkgs; };
  cert = import ../lib/test-cert.nix { inherit pkgs; } {
    subject = "test.invalid";
    names = [ "*.test.invalid" ];
  };
  takeBox =
    name:
    { config, ... }:
    {
      imports = [
        ./box.nix
        ../modules/storage/store.nix
        ../modules/storage/postgres-follow.nix
        ../modules/photos/ente.nix
      ];
      config = {
        # what fleet/policy.json says on a real box
        dd.failover = true;
        dd.verify.enable = false;
        users.groups.dd-verify = { };
        dd.store = {
          enable = true;
          # as every box has it: what its disk has room for
          identity = "/etc/age-identity";
          agePlugins = [ ];
          peers.${other name} = {
            url = "http://store.${other name}.test.invalid";
            inherit (ages.${other name}) recipient;
          };
          buckets.postgres-wal.owner = "postgres";
        };
        dd.postgres.follow = {
          enable = true;
          endpoint = "http://127.0.0.1:3910";
          envFile = "/var/lib/dd-store/keys/postgres-wal.env";
          identity = "/etc/age-identity";
          agePlugins = [ ];
          recipients = builtins.mapAttrs (_: k: k.recipient) ages;
        };
        environment.etc."age-identity" = {
          text = ages.${name}.identity;
          mode = "0400";
        };
        # what box/placed.nix gives the placed services' units
        systemd.services = {
          postgresql.unitConfig.ConditionPathExists = "/run/dd-placed/postgresql";
          postgresql.serviceConfig.ExecCondition = "${handing "postgresql"}";
          postgresql-setup.unitConfig.ConditionPathExists = "/run/dd-placed/postgresql";
          postgresql-setup.serviceConfig.ExecCondition = "${handing "postgresql"}";
          ente.unitConfig.ConditionPathExists = "/run/dd-placed/ente-museum";
          ente.serviceConfig.ExecCondition = "${handing "ente-museum"}";
          ente-health.unitConfig.ConditionPathExists = "/run/dd-placed/ente-museum";
          ente-health.serviceConfig.ExecCondition = "${handing "ente-museum"}";
        };
        systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
        services.ente.web.enable = lib.mkForce false;
        # what the web module sets for museum when it is on
        services.ente.api.settings = {
          apps = {
            accounts = "https://accounts.test.invalid";
            cast = "https://cast.test.invalid";
            public-albums = "https://albums.test.invalid";
          };
          webauthn = {
            rpid = "accounts.test.invalid";
            rporigins = [ "https://accounts.test.invalid" ];
          };
        };
        services.nginx.enable = true;
        # what the gate's module has, which this test leaves out
        services.nginx.appendHttpConfig = ''
          map $http_cookie $dd_cookie_stripped { default ""; }
        '';
        services.nginx.virtualHosts = lib.mkMerge [
          (lib.genAttrs
            (map (h: "${h}.test.invalid") [
              "api"
              "accounts"
              "albums"
              "cast"
              "photos"
              "locker"
              "photos-store"
            ])
            (_: {
              useACMEHost = lib.mkForce null;
              sslCertificate = "${cert}/cert.pem";
              sslCertificateKey = "${cert}/key.pem";
            })
          )
          {
            "locker.test.invalid".locations."/".root = lib.mkForce "/var/empty";
            # no certificate for the store's own name: plain http here
            "store.${name}.test.invalid" = {
              useACMEHost = lib.mkForce null;
              forceSSL = lib.mkForce false;
              onlySSL = lib.mkForce false;
              listen = lib.mkForce [
                {
                  addr = "0.0.0.0";
                  port = 80;
                }
              ];
            };
          }
        ];
        networking.extraHosts = ''
          192.168.1.1 store.a.test.invalid
          192.168.1.2 store.b.test.invalid
        '';
        environment.systemPackages = [
          pkgs.curl
          pkgs.jq
        ];
      };
    };
in
{
  name = "takeover";
  helpers = true;
  node.specialArgs = { inherit self; };
  # ente.nix pins museum by an overlay
  node.pkgsReadOnly = false;
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  defaults.virtualisation.diskSize = 4096;
  nodes = {
    a = takeBox "a";
    b = takeBox "b";
  };
}
