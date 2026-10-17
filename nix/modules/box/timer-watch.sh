# shellcheck shell=bash
# A calendar timer that has lost its next run is restarted. node2's nightly
# backup timer came out of the release switches of 2026-10-12 active, with
# no next run (`systemctl list-timers` NEXT "-"), and no backup ran for
# five days. Restarting a timer gives it its next run again.
#
# Only timers with an OnCalendar: one with only OnActiveSec runs once by
# design and has nothing next. Nor one whose service is running now: it
# shows no next run until the run ends.
set -u
n=0
while read -r timer _; do
  [ -n "$timer" ] || continue
  cal=$(systemctl show -P TimersCalendar "$timer")
  case "$cal" in *OnCalendar=*) ;; *) continue ;; esac
  [ "$(systemctl show -P ActiveState "$timer")" = active ] || continue
  [ -z "$(systemctl show -P NextElapseUSecRealtime "$timer")" ] || continue
  unit=$(systemctl show -P Triggers "$timer")
  case "$(systemctl show -P ActiveState "$unit")" in active | activating | deactivating) continue ;; esac
  echo "$timer: no next run; restarting it"
  systemctl restart "$timer" && n=$((n + 1))
done < <(systemctl list-units --type=timer --all --plain --no-legend)
out=/var/lib/dd-facts/timers.prom
printf 'dd_timers_restarted %d\n' "$n" > "$out.new" && mv "$out.new" "$out"
exit 0
