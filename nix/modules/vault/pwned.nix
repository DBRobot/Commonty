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
    gen="$(date +%Y%m%d)"
    next="${dir}/gen-$gen"
    mkdir -p "$next"
    cd "$next"
    # one line per range: a curl config, so a million fetches run in one
    # process, 64 at a time; a range already here from a run that stopped
    # partway is not fetched again
    for i in $(seq 0 1048575); do
      p=$(printf '%05X' "$i")
      [ -s "$p" ] || printf 'url = "https://api.pwnedpasswords.com/range/%s"\noutput = "%s"\n' "$p" "$p"
    done > ../fetch.cfg
    if [ -s ../fetch.cfg ]; then
      curl --parallel --parallel-max 64 --fail --silent --show-error --retry 5 \
        --user-agent "commonty-pwned-mirror" -K ../fetch.cfg
    fi
    rm -f ../fetch.cfg
    n=$(find . -maxdepth 1 -type f | wc -l)
    [ "$n" -eq 1048576 ] || { echo "only $n of 1048576 ranges; keeping the old list" >&2; exit 1; }
    ln -sfn "gen-$gen" ../current.new
    mv -T ../current.new ../current
    # the older lists: public data, fetched again whenever it is wanted
    for g in ../gen-*; do [ "$g" = "../gen-$gen" ] || rm -rf "$g"; done
  '';
in
{
  options.dd.vault.pwnedDir = lib.mkOption {
    type = lib.types.str;
    default = "/vault/pwned";
    description = "where the breach list lives (about 40 GB)";
  };

  config = lib.mkIf cfg.enable {
    users.users.dd-pwned = {
      isSystemUser = true;
      group = "nginx";
    };
    systemd.services.dd-pwned = {
      description = "Fetch the Pwned Passwords list";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      unitConfig.RequiresMountsFor = dir;
      path = [
        pkgs.curl
        pkgs.coreutils
        pkgs.findutils
      ];
      serviceConfig = {
        Type = "oneshot";
        User = "dd-pwned";
        Group = "nginx";
        ProtectSystem = "strict";
        ReadWritePaths = [ dir ];
        PrivateTmp = true;
        NoNewPrivileges = true;
        UMask = "0027";
        Nice = 19;
        IOSchedulingClass = "idle";
        ExecStartPre = "+${pkgs.coreutils}/bin/install -d -m 0750 -o dd-pwned -g nginx ${dir}";
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
    services.nginx.virtualHosts."vault.${config.dd.domain}".locations."~ ^/pwned/range/([0-9A-Fa-f]{5})$" =
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
