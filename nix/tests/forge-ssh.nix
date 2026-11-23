# Git over ssh from outside the fleet's network: the forge's own sshd
# (nix/modules/forge/ssh-door.nix) next to a box's admin sshd, driven as a
# stranger would drive it. Only the forgejo user gets in, by key, to its
# forced command: no shell, exec, pty or forwarding, no password, and its
# host key is not the admin sshd's. The tunnel's rules (door-ingress.nix)
# are checked at evaluation: one ssh route, to this port, and no other name
# or port 22 anywhere.
{ pkgs, ... }:
let
  lib = pkgs.lib;
  port = 2223;
  base = "commonty.test";
  outside = map (h: "${h}.${base}") [
    "home"
    "accounts"
    "code"
    "demo-files"
  ];
  rules =
    sshPort:
    import ../lib/door-ingress.nix {
      inherit lib base outside sshPort;
      tunnelPort = 8443;
    };
  on = rules port;
  isSsh = r: lib.hasPrefix "ssh:" r.service;
  sshRules = lib.filter isSsh on;
  problems =
    lib.optional (lib.length sshRules != 1) "not exactly one ssh route"
    ++
      lib.optional
        (
          sshRules != [ ]
          && lib.head sshRules != {
            hostname = "ssh.${base}";
            service = "ssh://127.0.0.1:${toString port}";
          }
        )
        "the ssh route is not ssh.<domain> to the forge's sshd"
    ++ lib.optional (lib.last on != { service = "http_status:404"; }) "the rules do not end in 404"
    ++ map (r: "${r.hostname} does not go to the tunnel's listener") (
      lib.filter (
        r: r ? hostname && r.hostname != "ssh.${base}" && r.service != "https://127.0.0.1:8443"
      ) on
    )
    ++ lib.optional (lib.any (r: lib.hasSuffix ":22" r.service) on) "a rule goes to port 22"
    ++ lib.optional (lib.any isSsh (rules null)) "an ssh route without a port set";
in
assert lib.assertMsg (problems == [ ]) "tunnel rules: ${lib.concatStringsSep "; " problems}";
{
  name = "forge-ssh";
  nodes.box =
    { pkgs, ... }:
    {
      imports = [ ../modules/forge/ssh-door.nix ];
      dd.forgejo.sshDoor = {
        inherit port;
        keyDir = "/var/lib/dd-forge-sshd";
      };
      # the forge's user, as services.forgejo makes it
      users.users.forgejo = {
        isSystemUser = true;
        group = "forgejo";
        home = "/var/lib/forgejo";
        createHome = true;
        shell = pkgs.bash;
      };
      users.groups.forgejo = { };
      users.users.alice.isNormalUser = true;
      # the admin sshd, on 22 as on a real box
      services.openssh.enable = true;
      environment.systemPackages = [ pkgs.openssh ];
    };
  scriptEnv.port = port;
}
