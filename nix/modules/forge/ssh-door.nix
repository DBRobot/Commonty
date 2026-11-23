# Git over ssh from outside the fleet's network: a second sshd, the forge's
# own, that the Cloudflare tunnel's `ssh.<domain>` route ends at
# (modules/gate/public.nix, nix/lib/door-ingress.nix). The box's admin sshd on
# 22 is never behind the tunnel: it stays on the fleet's network, and this
# one can do one thing, run the `forgejo serv` command that Forgejo writes
# into the forgejo user's authorized_keys (a key line per member key, each
# forcing that command with no pty and no forwarding).
{
  config,
  options,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.forgejo.sshDoor;
  sshd = "${pkgs.openssh}/bin/sshd";
  hostKey = "${cfg.keyDir}/ssh_host_ed25519_key";
  conf = pkgs.writeText "dd-forge-sshd.conf" ''
    ListenAddress 127.0.0.1:${toString cfg.port}
    HostKey ${hostKey}
    PidFile none

    # the forge's user and nobody else, by key alone
    AllowUsers forgejo
    PermitRootLogin no
    AuthenticationMethods publickey
    PubkeyAuthentication yes
    PasswordAuthentication no
    KbdInteractiveAuthentication no
    GSSAPIAuthentication no
    PermitEmptyPasswords no
    AuthorizedKeysFile %h/.ssh/authorized_keys
    AuthorizedKeysCommand none
    # the account is locked (no password), which sshd refuses without PAM
    UsePAM yes

    # a git transport and nothing else: no tty, no forwarding of any kind
    PermitTTY no
    DisableForwarding yes
    AllowTcpForwarding no
    AllowStreamLocalForwarding no
    AllowAgentForwarding no
    X11Forwarding no
    PermitTunnel no
    GatewayPorts no
    PermitOpen none
    PermitUserEnvironment no
    PermitUserRC no
    AcceptEnv GIT_PROTOCOL
    MaxSessions 4
    PrintMotd no
    Banner none
    Compression no

    # every connection comes from the tunnel, so these limits are global
    MaxAuthTries 3
    LoginGraceTime 20
    MaxStartups 10:30:30
    ClientAliveInterval 30
    ClientAliveCountMax 4
    UseDNS no
    StrictModes yes
    LogLevel VERBOSE
  '';
in
{
  options.dd.forgejo.sshDoor = {
    port = lib.mkOption {
      type = lib.types.nullOr lib.types.port;
      default =
        if options.dd ? public && config.dd.public.ssh != null then config.dd.public.ssh.port else null;
      defaultText = "dd.public.ssh's port, when it is on";
      description = "where the forge's own sshd listens, on 127.0.0.1 only; null: no such sshd";
    };
    keyDir = lib.mkOption {
      type = lib.types.str;
      default = "${config.dd.forgejo.stateDir}/sshd";
      description = "the sshd's own host key, made once on the box that holds the forge and kept with the forge's files, so a box taking it over answers with the same key; never the admin sshd's";
    };
  };

  config = lib.mkIf (cfg.port != null) {
    systemd.services.dd-forge-sshd = {
      description = "The forge's own sshd, for git over ssh through the tunnel";
      wantedBy = [ "multi-user.target" ];
      after = [
        "network.target"
        "zfs-datasets.service"
      ];
      unitConfig.RequiresMountsFor = cfg.keyDir;
      path = [
        pkgs.openssh
        pkgs.coreutils
      ];
      serviceConfig = {
        # root: sshd drops to forgejo itself after authenticating
        ExecStartPre = [
          (pkgs.writeShellScript "dd-forge-sshd-key" ''
            set -eu
            install -d -m 0700 -o root -g root ${cfg.keyDir}
            [ -e ${hostKey} ] || ssh-keygen -q -t ed25519 -N "" -C dd-forge-sshd -f ${hostKey}
            chmod 600 ${hostKey}
          '')
          "${sshd} -t -f ${conf}"
        ];
        ExecStart = "${sshd} -D -e -f ${conf}";
        Restart = "always";
        RestartSec = 5;
        KillMode = "process";
        # sshd needs its users, PAM and setuid(forgejo), little else
        PrivateTmp = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        RestrictNamespaces = true;
        LockPersonality = true;
      };
    };
  };
}
