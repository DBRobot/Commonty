# shellcheck shell=bash
# The runner's unit as it is on disk now, against the one its run started
# from. MODE=started records it (ExecStartPre); MODE=check restarts the
# runner when they differ and no job is running (dd-runner-restart).
set -eu
state=/run/dd-runner-restart/$UNIT
fingerprint() { systemctl cat "$UNIT" 2>/dev/null | sha256sum | cut -d' ' -f1; }

if [ "$MODE" = started ]; then
  mkdir -p "${state%/*}"
  fingerprint >"$state"
  exit 0
fi

# not running, or starting or stopping: nothing to do now, next time
if [ "$(systemctl show -P ActiveState "$UNIT")" != active ]; then
  echo "$UNIT is not running; next time"
  exit 0
fi
# a runner started before this was here has no record: it is not current
if [ -e "$state" ] && [ "$(cat "$state")" = "$(fingerprint)" ]; then
  exit 0
fi
# a job is processes in the unit besides the runner's own
main=$(systemctl show -P MainPID "$UNIT")
cgroup=$(systemctl show -P ControlGroup "$UNIT")
jobs=$(grep -cvx "$main" "/sys/fs/cgroup$cgroup/cgroup.procs" || true)
if [ "$jobs" -gt 0 ]; then
  echo "$UNIT changed; a job is running ($jobs processes), next time"
  exit 0
fi
echo "$UNIT changed and holds no job: restarting it"
systemctl restart --no-block "$UNIT"
