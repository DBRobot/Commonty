# shellcheck shell=bash
# A unit as it is on disk now, against the one its run started from.
# `started UNIT SANDBOX` records it (ExecStartPre); `check ENTRY...`
# restarts each unit whose definition differs once it holds no job
# (dd-runner-restart). An ENTRY is UNIT|HOW|LABEL|LIMIT|SANDBOX|DRAIN:
#   HOW: what a job is. `cgroup`, processes in the unit besides its main one
#     (host-mode jobs); `docker`, a running container carrying LABEL (the
#     members' runner's, not any container on the box).
#   LIMIT: seconds a change waits for no job. Past it the unit drains and
#     restarts all the same: a member keeping jobs running back to back put
#     off a new sandbox forever.
#   SANDBOX: a hash of what of its config is the sandbox. A change there
#     drains and restarts on the next pass, cancelling the jobs under way.
#   DRAIN: units (comma separated) stopped first, each a drain, and started
#     again after: docker's, the members' runner.
# FACTS, when there, gets how long each change has waited.
set -eu
mode=$1
shift
dir=/run/dd-runner-restart
fingerprint() { systemctl cat "$1" 2>/dev/null | sha256sum | cut -d' ' -f1; }
state() { echo "$dir/$1"; }
active() { systemctl show -P ActiveState "$1"; }

if [ "$mode" = started ]; then
  mkdir -p "$dir"
  fingerprint "$1" >"$(state "$1")"
  printf '%s\n' "${2:-}" >"$(state "$1").sandbox"
  rm -f "$(state "$1").pending" "$(state "$1").held"
  exit 0
fi

# the number of jobs in a unit, or nothing when it cannot be told
jobs() {
  case $2 in
  cgroup)
    local main cgroup
    main=$(systemctl show -P MainPID "$1")
    cgroup=$(systemctl show -P ControlGroup "$1")
    grep -cvx "$main" "/sys/fs/cgroup$cgroup/cgroup.procs" || true
    ;;
  docker)
    local ids
    ids=$(docker ps -q --filter "label=$3") || return 0
    echo "$ids" | grep -c . || true
    ;;
  esac
}

# a stop asked for: wait until the unit takes no new job (it is past active)
stopping() {
  for _ in $(seq 30); do
    [ "$(active "$1")" = active ] || return 0
    sleep 1
  done
}

# the jobs under way end now: their containers go, and their runner tells
# the forge the job failed
cancel() {
  local ids
  ids=$(docker ps -q --filter "label=$1") || return 0
  [ -z "$ids" ] && return 0
  echo "cancelling the jobs under way: $(echo "$ids" | grep -c .) container(s)"
  # shellcheck disable=SC2086
  docker kill $ids >/dev/null || true
}

# restart a unit whatever it holds: its drain units stop first (each takes no
# new job and lets its jobs end, up to its shutdown timeout), then it
# restarts and they start again. With no drain units its own stop drains.
force() {
  local unit=$1 drains=$2 why=$3 label=$4 kill=$5 d busy=
  for d in ${drains//,/ }; do
    case $(active "$d") in
    active)
      echo "$unit: $why; draining $d first"
      touch "$(state "$d").held"
      systemctl stop --no-block "$d"
      stopping "$d"
      busy=1
      ;;
    activating | deactivating | reloading) busy=1 ;;
    esac
  done
  if [ -n "$busy" ]; then
    [ "$kill" = 1 ] && cancel "$label"
    echo "$unit: $why; restarting once ${drains//,/ and } has stopped"
    return
  fi
  echo "$unit: $why; restarting it"
  systemctl restart --no-block "$unit"
  if [ -z "$drains" ] && [ "$kill" = 1 ]; then
    stopping "$unit"
    cancel "$label"
  fi
  resume "$drains"
}

# what was stopped for a restart starts again (also when the restart came
# some other way)
resume() {
  local d
  for d in ${1//,/ }; do
    if [ -e "$(state "$d").held" ]; then
      rm -f "$(state "$d").held"
      systemctl start --no-block "$d"
    fi
  done
}

check() {
  # not running, or starting or stopping: nothing to do now, next time
  if [ "$(active "$unit")" != active ]; then
    echo "$unit is not running; next time"
    return
  fi
  # stopped for another unit's restart, which starts it again
  if [ -e "$(state "$unit").held" ]; then
    echo "$unit is held for another unit's restart; next time"
    return
  fi
  # the sandbox changed, or which one it runs is not known (no record: it
  # started before this was here, or the record went): no job runs on it
  # past this pass. Each start records it, a boot's included.
  local was
  was=$(cat "$(state "$unit").sandbox" 2>/dev/null || true)
  if [ -n "$sandbox" ] && [ "$was" != "$sandbox" ]; then
    [ -s "$pending" ] || echo "$now" >"$pending"
    force "$unit" "$drains" "its sandbox changed or is not known" "$label" 1
    return
  fi
  # a unit started before this was here has no record: it is not current
  if [ -e "$(state "$unit")" ] && [ "$(cat "$(state "$unit")")" = "$(fingerprint "$unit")" ]; then
    rm -f "$pending"
    resume "$drains"
    return
  fi
  [ -s "$pending" ] || echo "$now" >"$pending"
  local waited=$((now - $(cat "$pending")))
  if [ "$waited" -ge "$limit" ]; then
    force "$unit" "$drains" "changed ${waited}s ago, past its limit (${limit}s)" "$label" 0
    return
  fi
  local n
  n=$(jobs "$unit" "$how" "$label")
  if [ -z "$n" ]; then
    echo "$unit changed; cannot tell whether a job is running, next time"
    return
  fi
  if [ "$n" -gt 0 ]; then
    echo "$unit changed; a job is running ($n $how), next time (waited ${waited}s of ${limit}s)"
    return
  fi
  echo "$unit changed and holds no job: restarting it"
  systemctl restart --no-block "$unit"
}
now=$(date +%s)
mkdir -p "$dir"
facts=
for entry in "$@"; do
  IFS='|' read -r unit how label limit sandbox drains <<<"$entry"
  pending=$(state "$unit").pending
  check
  waited=0
  [ -s "$pending" ] && waited=$((now - $(cat "$pending")))
  facts="${facts}dd_runner_restart_pending_seconds{unit=\"${unit//\\x2d/-}\"} $waited"$'\n'
done
if [ -n "${FACTS:-}" ] && [ -d "$FACTS" ]; then
  printf '%s' "$facts" >"$FACTS/runner-restart.prom.new" && mv "$FACTS/runner-restart.prom.new" "$FACTS/runner-restart.prom"
fi
