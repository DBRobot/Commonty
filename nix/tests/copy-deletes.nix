# With dd.store.copyDeletes on: a box's prune reaches its copies, on the plan's
# and the agreed state's say, and a bad box's key at another store deletes
# nothing that is not its to write. A stand-in answers for dd-control.
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
  storeBox =
    name:
    { lib, ... }:
    {
      dd.verify.enable = false;
      imports = [
        ./box.nix
        ../modules/storage/store.nix
      ];
      dd.store = {
        enable = true;
        copyDeletes = true;
        copyDeleteAfter = "1ms";
        capacity = "1G";
        identity = "/etc/age-identity";
        agePlugins = [ ];
        peers.${other name} = {
          url = "http://store.${other name}.test.invalid:3443";
          inherit (ages.${other name}) recipient;
        };
        # each box's own, kept on both
        buckets.ours-a.writer = "a";
        buckets.ours-b.writer = "b";
        # a placed service's: written where the plan or its lease says
        buckets.mailbox.service = "dd-mail";
      };
      environment.etc."age-identity" = {
        text = ages.${name}.identity;
        mode = "0400";
      };
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
      environment.systemPackages = [
        pkgs.awscli2
        pkgs.age
        pkgs.curl
        pkgs.python3
      ];
      systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
    };
in
{
  name = "copy-deletes";
  helpers = true;
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1024;
  defaults.virtualisation.diskSize = 2048;
  nodes = {
    a = storeBox "a";
    b = storeBox "b";
  };
}
