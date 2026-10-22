# Each box backs itself up into its own store, and the copy goes to the
# other's store; a box that has lost everything gets its files back from
# the other: the restore path is the one that matters, not the backup path.
{ pkgs, self, ... }:
let
  # test keys only: on a real box this is its TPM-held secrets key
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
  password = name: pkgs.writeText "restic-password" "${name}-password\n";
  storageBox =
    name:
    { lib, ... }:
    {
      # the gate plays no part here: without it, a change to the gate
      # is not a reason to run this again
      dd.verify.enable = false;
      imports = [
        ./box.nix
        ../modules/storage/store.nix
        ../modules/storage/backup.nix
        ../modules/box/timer-watch.nix
      ];
      dd.store = {
        enable = true;
        capacity = "2G";
        identity = "/etc/age-identity";
        agePlugins = [ ];
        boxes = [
          "a"
          "b"
        ];
        peers.${other name} = {
          url = "http://store.${other name}.test.invalid:3443";
          inherit (ages.${other name}) recipient;
        };
      };
      environment.etc."age-identity" = {
        text = ages.${name}.identity;
        mode = "0400";
      };
      # no certificate in a test: the store's name over plain http
      services.nginx.enable = true;
      services.nginx.virtualHosts."store.${name}.test.invalid" = {
        useACMEHost = lib.mkForce null;
        onlySSL = lib.mkForce false;
        listen = lib.mkForce [
          {
            addr = "0.0.0.0";
            port = 3443;
          }
        ];
      };
      networking.extraHosts = ''
        192.168.1.1 store.a.test.invalid
        192.168.1.2 store.b.test.invalid
      '';
      # the repository and its key are the store's (backup.nix defaults)
      dd.backup = {
        paths = [ "/srv/state" ];
        passwordFile = password name;
      };
      environment.systemPackages = [
        pkgs.restic
        pkgs.awscli2
      ];
      systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
    };
in
{
  name = "backup";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1536;
  defaults.virtualisation.cores = 2; # services start in parallel instead of queueing on one thread
  defaults.virtualisation.diskSize = 4096;
  nodes = {
    a = storageBox "a";
    b = storageBox "b";
  };
  scriptEnv = {
    a_password = password "a";
  };
}
