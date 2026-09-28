# The breach list, kept on this box: every password hash Have I Been Pwned
# knows, as the 16^5 range files its api serves. The web vault's breach
# checks ask here instead, so which passwords a member checks never leaves
# the box, not even as a five-character prefix. A public list, so not
# backed up: a lost copy is fetched again.
{
  config,
  options,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.vault;
  dir = cfg.pwnedDir;

  fetch = pkgs.writeShellScript "dd-pwned-fetch" ''
    set -eu
    cd ${dir}
    # a run that stopped partway goes on where it was; otherwise a new list
    live=$(readlink current 2>/dev/null || true)
    next=$(ls -d gen-* 2>/dev/null | grep -vx "$live" | sort | tail -1 || true)
    [ -n "$next" ] || next="gen-$(date +%Y%m%d)"
    mkdir -p "$next"
    cd "$next"
    # a curl config, so a million fetches run in one process, 64 at a time;
    # a range already here is not fetched again
    ${pkgs.python3}/bin/python3 ${./pwned-list.py} . > ../fetch.cfg
    if [ -s ../fetch.cfg ]; then
      curl --parallel --parallel-max 64 --fail --silent --show-error --retry 5 \
        --user-agent "commonty-pwned-mirror" -K ../fetch.cfg
    fi
    rm -f ../fetch.cfg
    n=$(find . -maxdepth 1 -type f | wc -l)
    [ "$n" -eq 1048576 ] || { echo "only $n of 1048576 ranges; keeping the old list" >&2; exit 1; }
    ln -sfn "$next" ../current.new
    mv -T ../current.new ../current
    # the older lists: public data, fetched again whenever it is wanted
    for g in ../gen-*; do [ "$g" = "../$next" ] || rm -rf "$g"; done
  '';
in
{
  options.dd.vault.pwnedDir = lib.mkOption {
    type = lib.types.str;
    default = "/vault/pwned";
    description = "where the breach list lives (about 90 GB: a million ranges of 80-100 KB)";
  };

  config = lib.mkIf cfg.enable {
    # public data: readable by anyone on the box, nginx included
    users.users.dd-pwned = {
      isSystemUser = true;
      group = "dd-pwned";
    };
    users.groups.dd-pwned = { };
    # made before the unit starts: its sandbox names the directory, and a
    # directory made from inside the unit comes too late (the first start
    # on node1 failed with "Failed to set up mount namespacing")
    systemd.tmpfiles.rules = [ "d ${dir} 0755 dd-pwned dd-pwned -" ];
    systemd.services.dd-pwned = {
      description = "Fetch the Pwned Passwords list";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      unitConfig.RequiresMountsFor = dir;
      path = [
        pkgs.curl
        pkgs.coreutils
        pkgs.findutils
        pkgs.gnugrep
      ];
      serviceConfig = {
        Type = "oneshot";
        User = "dd-pwned";
        Group = "dd-pwned";
        ProtectSystem = "strict";
        ReadWritePaths = [ dir ];
        PrivateTmp = true;
        NoNewPrivileges = true;
        UMask = "0022";
        Nice = 19;
        IOSchedulingClass = "idle";
      };
      script = "exec ${fetch}";
    };
    systemd.timers.dd-pwned = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = "Sun 04:00";
        RandomizedDelaySec = "2h";
        Persistent = true;
      };
    };

    # /pwned/range/ABCDE on the vault's host, inside only (the vault's
    # location rules in vaultwarden.nix)
    services.nginx.virtualHosts."vault.${config.dd.domain}".locations."~ \"^/pwned/range/([0-9A-Fa-f]{5})$\"" =
      {
        extraConfig =
          lib.optionalString (options.dd ? public && config.dd.public.enable) ''
            if ($dd_inside = 0) { return 444; }
          ''
          + ''
            access_log off;
            default_type text/plain;
            add_header Cache-Control "private, max-age=86400";
            alias ${dir}/current/$1;
          '';
      };
  };
}
