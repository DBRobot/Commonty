# The network's control server takes over (failover: net/follow.nix): a runs
# headscale at epoch 1, with a user and a key on it; its database, sealed to
# the boxes able to run it (a and b), goes to b's and c's stores. b takes
# epoch 2 with what a had and a server key of its own; c keeps a copy and
# cannot read it. The flags dd-control would raise are raised by hand.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  # test keys only: what each box's TPM-held secrets key is on a real one
  handing = import ../lib/not-handing.nix { inherit pkgs; };
  ages = {
    a = {
      identity = "AGE-SECRET-KEY-1RYQV6YMT4DPUURXLGDHLEP6P4QLM4NYQ4Z4E594SKSVYTXHZTF6QAJ94TJ";
      recipient = "age1gc25m409smgcklvytkksgew4ycjky4xtwvarsynwjxa9eg60jgpqfrlgp8";
    };
    b = {
      identity = "AGE-SECRET-KEY-18PLY0LS6RNEJLKPAYQRU6FD35UMDPS7WLXV94ASZ4G286NWRKSJQQPJL2V";
      recipient = "age1qg66feuknweacfxtff0mfejky0xzuk8d23zssz763lpmptxxquessgd8hd";
    };
    # a box that keeps a copy and may not run the server
    c = {
      identity = "AGE-SECRET-KEY-143L4XFYPXPHRAJ9LD34SA9EEES229HW6QWG4VLKZTKHJ5252UQZSW5U0XG";
      recipient = "age1t4rf2rewl4ndftdle2npauuu49xad84jp4fkt92th96ld5xct99sjx3jv4";
    };
  };
  followBox =
    name:
    { config, ... }:
    {
      imports = [
        ./box.nix
        ../modules/storage/store.nix
        ../modules/net/headscale.nix
        # how a box joins the network, which the server's module sets
        ../modules/net/box.nix
      ];
      config = {
        # what fleet/policy.json says on a real box
        dd.failover = true;
        dd.headscale = {
          enable = true;
          url = "http://127.0.0.1:8085";
          behindNginx = false;
          follow = {
            # who may hold its lease: a and b, not c
            recipients = {
              a = ages.a.recipient;
              b = ages.b.recipient;
            };
            identity = "/etc/age-identity";
            agePlugins = [ ];
          };
        };
        # no internet here for Tailscale's relay map: one of its own, with a
        # relay that is never reached (headscale wants at least one)
        services.headscale.settings.derp = {
          urls = lib.mkForce [ ];
          paths = [
            (pkgs.writeText "derp.yaml" ''
              regions:
                900:
                  regionid: 900
                  regioncode: test
                  regionname: test
                  nodes:
                    - name: 900a
                      regionid: 900
                      hostname: derp.test.invalid
                      stunport: -1
            '')
          ];
        };
        dd.store = {
          enable = true;
          identity = "/etc/age-identity";
          agePlugins = [ ];
          peers = lib.mapAttrs (n: k: {
            url = "http://store.${n}.test.invalid";
            inherit (k) recipient;
          }) (lib.filterAttrs (n: _: n != name) ages);
        };
        environment.etc."age-identity" = {
          text = ages.${name}.identity;
          mode = "0400";
        };
        # what box/placed.nix gives the placed service's units
        systemd.services =
          lib.genAttrs
            [
              "headscale"
              "headscale-seed"
              "dd-headscale-copy"
              "dd-headscale-handover"
            ]
            (u: {
              unitConfig.ConditionPathExists = "/run/dd-placed/headscale";
              serviceConfig = lib.optionalAttrs (u != "dd-headscale-handover") {
                ExecCondition = "${handing "headscale"}";
              };
            });
        systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
        services.nginx.enable = true;
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
          192.168.1.3 store.c.test.invalid
        '';
        environment.systemPackages = [
          pkgs.curl
          pkgs.jq
          pkgs.age
          pkgs.awscli2
          pkgs.sqlite
        ];
      };
    };
in
{
  name = "net-follow";
  helpers = true;
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1536;
  nodes = {
    a = followBox "a";
    b = followBox "b";
    c = followBox "c";
  };
}
