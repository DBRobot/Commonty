# The forge takes over (failover: forge/follow.nix): a runs postgres and
# forgejo at epoch 1 on its ZFS dataset; repositories are made and one is
# deleted again; the copies, out of a snapshot with the list of what it held,
# go to b's store with postgres's archive. b takes epoch 2: what a kept is
# there, what a deleted is not, b's own earlier files are set aside, and a
# commit on b goes through its hooks. The flags dd-control would raise are
# raised by hand.
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
  # what box/placed.nix gives the placed services' units
  forgeUnits = [
    "dd-forge-dirs"
    "forgejo-secrets"
    "forgejo"
    "forgejo-admin"
    "forgejo-protection"
    "dd-forge-rules"
    "dd-ci-cancel"
    "dd-ci-resume"
    "dd-forget-forge"
    "dd-forge-copy"
  ];
  followBox =
    name:
    { config, ... }:
    {
      imports = [
        ./box.nix
        ../modules/storage/store.nix
        ../modules/storage/postgres-follow.nix
        ../modules/forge/forgejo.nix
      ];
      config = {
        # what fleet/policy.json says on a real box
        dd.failover = true;
        dd.forgejo.admin = "tester";
        # the forge's dataset, in a pool on a disk of its own, as a box's
        # bulk pool; made by a stand-in for zfs-datasets
        dd.box.bulkPool = "tank";
        boot.supportedFilesystems = [ "zfs" ];
        networking.hostId = if name == "a" then "aaaaaaaa" else "bbbbbbbb";
        virtualisation.emptyDiskImages = [ 2048 ];
        systemd.services = {
          zfs-datasets = {
            wantedBy = [ "multi-user.target" ];
            path = [ config.boot.zfs.package ];
            serviceConfig = {
              Type = "oneshot";
              RemainAfterExit = true;
            };
            script = ''
              zpool list tank >/dev/null 2>&1 || zpool create -f -m none tank /dev/vdb
              zfs list tank/enc/forgejo >/dev/null 2>&1 \
                || zfs create -p -o mountpoint=${config.dd.forgejo.stateDir} tank/enc/forgejo
            '';
          };
        }
        // lib.genAttrs forgeUnits (_: {
          unitConfig.ConditionPathExists = "/run/dd-placed/forgejo";
        })
        // lib.genAttrs [ "postgresql" "postgresql-setup" ] (_: {
          unitConfig.ConditionPathExists = "/run/dd-placed/postgresql";
        });
        dd.store = {
          enable = true;
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
        systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
        services.nginx.enable = true;
        # the forge's own names want the fleet's certificate: here the
        # placeholder acme makes before any order, which never reaches out
        security.acme = {
          acceptTerms = true;
          defaults.email = "test@test.invalid";
          certs."test.invalid".webroot = "/var/lib/acme/acme-challenge";
        };
        users.users.nginx.extraGroups = [ "acme" ];
        # what the gate's module has, which this test leaves out
        services.nginx.appendHttpConfig = ''
          map $http_cookie $dd_cookie_stripped { default ""; }
        '';
        services.nginx.virtualHosts."store.${name}.test.invalid" = {
          useACMEHost = lib.mkForce null;
          forceSSL = lib.mkForce false;
        };
        networking.extraHosts = ''
          192.168.1.1 store.a.test.invalid
          192.168.1.2 store.b.test.invalid
        '';
        environment.systemPackages = [
          pkgs.curl
          pkgs.jq
          pkgs.awscli2
        ];
      };
    };
in
{
  name = "forge-follow";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  defaults.virtualisation.diskSize = 4096;
  nodes = {
    a = followBox "a";
    b = followBox "b";
  };
  scriptEnv = {
    sock = "/run/forgejo/forgejo.sock";
    dir = "/vault/forgejo";
  };
}
