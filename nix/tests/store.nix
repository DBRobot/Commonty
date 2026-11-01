# Each box's own store (storage/store.nix): a garage of one with a secret
# nobody else has. a's bucket is copied to b's store with the key b handed
# a, sealed so only a could open it; b's copy holds what a wrote; a cannot
# read b's buckets it does not copy; the plan's copies decide where a
# bucket goes, a store nearly full refuses more; with a gone, b serves the
# copy.
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
        capacity = "1G";
        identity = "/etc/age-identity";
        agePlugins = [ ];
        peers.${other name} = {
          # at its own port, as on a box: the Host it signs carries the port
          url = "http://store.${other name}.test.invalid:3443";
          inherit (ages.${other name}) recipient;
        };
        buckets.photos = { };
        # big and able to wait: only where it leaves room there
        guarded.photos = [ "big/" ];
        copyFloorBytes = 1048576;
        # this box's own, kept here alone
        buckets.scratch.copies = 1;
        # a's alone to write (a box's backup): b keeps a copy, and never
        # sends one back
        buckets.ours.writer = "a";
        # kept in a's store, written there by both (the metric blocks)
        buckets.blocks = {
          copies = 1;
          shared = true;
          home = "a";
        };
      };
      environment.etc."age-identity" = {
        text = ages.${name}.identity;
        mode = "0400";
      };
      # no certificate in a test: the store's name over plain http, on
      # the port boxes reach it at
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
      # where dd-control writes the plan's copies
      systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
    };
in
{
  name = "store";
  helpers = true;
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1024;
  defaults.virtualisation.diskSize = 2048;
  nodes = {
    a = storeBox "a";
    b = storeBox "b";
  };
}
