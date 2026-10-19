# The VM tests' own helpers (nix/tests/lib/waits.py) against the races they
# exist for: a oneshot a timer keeps starting, a page curl writes past where
# grep stops reading, a socket that comes up late.
{ pkgs, ... }:
{
  name = "waits";
  helpers = true;
  nodes.box = {
    environment.systemPackages = [ pkgs.socat ];
    # a run takes two seconds and says which run it is; a timer starts one
    # every three, so a test's start often lands on a run under way
    systemd.services.slow = {
      serviceConfig.Type = "oneshot";
      script = ''
        n=$(( $(cat /var/lib/slow-runs 2>/dev/null || echo 0) + 1 ))
        echo "$n" > /var/lib/slow-runs
        echo "run $n began"
        sleep 2
        echo "run $n done"
      '';
    };
    systemd.timers.slow = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1s";
        OnUnitActiveSec = "3s";
        AccuracySec = "1ms";
      };
    };
    # a page of 2 MB with the word near its start: grep -q leaves early
    systemd.services.page = {
      wantedBy = [ "multi-user.target" ];
      script = ''
        mkdir -p /srv/page
        { echo hello-there; head -c 2000000 /dev/zero | tr '\0' x; } > /srv/page/index.html
        exec ${pkgs.python3}/bin/python3 -m http.server 8088 --directory /srv/page
      '';
    };
    # a unix socket that turns up five seconds after boot
    systemd.services.late-socket = {
      wantedBy = [ "multi-user.target" ];
      script = "sleep 5; exec ${pkgs.socat}/bin/socat UNIX-LISTEN:/run/late.sock,fork EXEC:${pkgs.coreutils}/bin/cat";
    };
  };
}
