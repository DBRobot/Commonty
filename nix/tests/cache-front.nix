# The nix cache through a store's door (storage/store.nix): a box away from
# the cache's home pushes and reads with nix's own s3 client, signed, over
# TLS on :3443, as dd release publish and every box's uploader do. Its url
# named the door "store.<box>.<domain>:3443" with &scheme=https; nix read
# the host as a scheme, curl sent plain http to the TLS port, and nginx
# answered every narinfo 400 (half a million of them from node2, 2026-10-25).
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
  # where a real box has its wildcard from acme
  cert = import ../lib/test-cert.nix { inherit pkgs; } {
    subject = "store.test.invalid";
    names = [
      "store.a.test.invalid"
      "store.b.test.invalid"
    ];
  };
  storeBox =
    name:
    { config, lib, ... }:
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
          url = "https://store.${other name}.test.invalid:3443";
          inherit (ages.${other name}) recipient;
        };
        # kept in a's store, written there by every box with its own key
        buckets.nix-cache = {
          copies = 1;
          shared = true;
          home = "a";
        };
      };
      environment.etc."age-identity" = {
        text = ages.${name}.identity;
        mode = "0400";
      };
      # the store's door as on a box, TLS on its own port; only the
      # certificate is the test's
      services.nginx.enable = true;
      services.nginx.virtualHosts."store.${name}.test.invalid" = {
        useACMEHost = lib.mkForce null;
        sslCertificate = "${cert}/cert.pem";
        sslCertificateKey = "${cert}/key.pem";
      };
      security.pki.certificateFiles = [ "${cert}/cert.pem" ];
      networking.extraHosts = ''
        192.168.1.1 store.a.test.invalid
        192.168.1.2 store.b.test.invalid
      '';
      # the url the box's uploader, agent and substituter are given
      environment.etc."cache-url".text = config.dd.store.at.nix-cache.s3Url;
      environment.etc."cache-env".text = config.dd.store.at.nix-cache.envFile;
      environment.systemPackages = [ pkgs.curl ];
      systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
    };
in
{
  name = "cache-front";
  helpers = true;
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1024;
  defaults.virtualisation.diskSize = 2048;
  nodes = {
    a = storeBox "a";
    b = storeBox "b";
  };
}
