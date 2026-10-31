# shellcheck shell=bash
# Under a delay lock on shutdown (shutdown-drain.nix). Waits for logind to say
# the box is going down, says it is leaving in the heartbeat as dd-drain does,
# and lets go once what it runs has moved, or can't, or DRAIN_SECS have passed.
# The file is gone at the next boot: a box that came back has left nothing.
set -u
facts=/var/lib/dd-facts
file=$facts/draining.prom
control=http://127.0.0.1:4195
carries=/etc/dd-placed/carries.json
placed=/run/dd-placed
box=$(cat /proc/sys/kernel/hostname)

rm -f "$file"

# the placed services this box runs now: their flags are up
running() {
  local svc
  [ -r "$carries" ] && for svc in $(jq -r 'keys[]' "$carries"); do
    if [ -e "$placed/$svc" ]; then echo "$svc"; fi
  done
  for svc in "$placed"/computer-*; do
    if [ -e "$svc" ]; then basename "$svc"; fi
  done
}

ask() { curl -fsS --max-time 3 "$control$1"; }

# once the plan has seen it leaving: is anything here that the plan moves?
# What moves on failure and what no other box can run stay in its plan, and
# stop with the box.
moves() {
  local plan svc
  plan=$(ask /plan) || return 0
  for svc in $(running); do
    jq -e --arg b "$box" --arg s "$svc" '[.boxes[$b][]?.service] | index($s) != null' <<<"$plan" >/dev/null || return 0
  done
  return 1
}

drain() {
  if [ -z "$(running)" ]; then
    echo "nothing here to hand on"
    return
  fi
  if ! ask /state >/dev/null; then
    echo "the control plane does not answer: nothing can be handed on"
    return
  fi
  install -d "$facts"
  echo "dd_box_draining 1" >"$file.new" && mv "$file.new" "$file"
  local start seen=0 now
  start=$(date +%s)
  while :; do
    now=$(($(date +%s) - start))
    if [ -z "$(running)" ]; then
      echo "handed on in ${now}s"
      return
    fi
    if [ "$now" -ge "$DRAIN_SECS" ]; then
      echo "gave up after ${now}s with $(running | tr '\n' ' ')still here"
      return
    fi
    if [ "$seen" = 0 ] && ask /state | jq -e --arg b "$box" '.boxes[$b].facts.draining > 0' >/dev/null; then
      seen=$now
    fi
    # (the plan's next round after the state said so)
    if [ "$seen" != 0 ] && [ $((now - seen)) -ge 8 ] && ! moves; then
      echo "the rest stays with this box: $(running | tr '\n' ' ')"
      return
    fi
    sleep 2
  done
}

exec 3< <(gdbus monitor --system --dest org.freedesktop.login1 --object-path /org/freedesktop/login1)
while read -r line <&3; do
  case "$line" in
    *"is owned by"*) echo "watching for a shutdown" ;;
    *".PrepareForShutdown (true)"*)
      drain
      exit 0
      ;;
    # a shutdown called off
    *".PrepareForShutdown (false)"*) rm -f "$file" ;;
  esac
done
# the bus went away: the unit starts this again
exit 1
