# shellcheck shell=bash
# Whether each service members use works on this box, as a fact: every unit
# it needs is running and, where it has one, its own address answers. LIST
# has a line per service, "<id> <unit,unit> <url or ->". Read by the uptime
# page through thanos (servers/gate/src/status.rs); this box asks no other.
set -u
out=$FACTS/uptime.prom
{
  echo "# HELP dd_service_up Whether a service members use works here: its units run and it answers."
  echo "# TYPE dd_service_up gauge"
  while read -r id units url; do
    up=1
    for u in ${units//,/ }; do
      systemctl is-active --quiet "$u.service" || up=0
    done
    if [ "$up" = 1 ] && [ "$url" != - ]; then
      code=$(curl -s -o /dev/null -m 5 -w '%{http_code}' "$url" || true)
      # an answer at all, short of the server's own failure
      if ! [ "$code" -ge 200 ] 2>/dev/null || [ "$code" -ge 500 ]; then up=0; fi
    fi
    echo "dd_service_up{service=\"$id\"} $up"
  done <"$LIST"
} >"$out.tmp" && mv "$out.tmp" "$out"
exit 0
