# shellcheck shell=bash
# What this box built, to the nix cache (storage/cache.nix). The post-build
# hook leaves a file per build in $SPOOL/queue; this sends until the queue
# stays empty for $IDLE seconds, files that land meanwhile included. A file
# goes once nix copy took its paths; one that failed or was cut off waits as
# <name>.try<N> (its mtime: when it may go again) and is tried again - nix
# copy skips what already got there - until $MAX_TRIES. A path over $MAX_MB
# is not sent on its own account; in a sent path's closure it goes too.
set -u
queue=$SPOOL/queue
BATCH=${BATCH:-20}
IDLE=${IDLE:-3}

listing() { find "$queue" -maxdepth 1 -type f -printf '%T@ %p\n' | sort -n; }

tries_of() {
  case $1 in
  *.try*) echo "${1##*.try}" ;;
  *) echo 0 ;;
  esac
}

failed() {
  local f=$1 n delay base
  n=$(($(tries_of "$f") + 1))
  base=${f%.try*}
  if [ "$n" -ge "$MAX_TRIES" ]; then
    echo "cache upload: gave up on $(tr '\n' ' ' <"$f")after $n tries" >&2
    rm -f "$f"
    return
  fi
  delay=$((RETRY_DELAY << (n - 1)))
  [ "$delay" -gt 1800 ] && delay=1800
  mv "$f" "$base.try$n" && touch -d "@$(($(date +%s) + delay))" "$base.try$n"
}

send() {
  local f p size sizes paths=() mib=0
  while read -r p; do
    [ -e "$p" ] || continue # a GC took it since
    case $p in *-nixos-disk-image) continue ;; esac
    paths+=("$p")
  done < <(sort -u "$@")
  if [ "${#paths[@]}" -gt 0 ]; then
    if ! sizes=$(nix path-info -s "${paths[@]}"); then
      for f in "$@"; do failed "$f"; done
      return
    fi
    mapfile -t paths < <(while read -r p size; do
      [ "$size" -le $((MAX_MB << 20)) ] && echo "$p"
    done <<<"$sizes")
  fi
  if [ "${#paths[@]}" -eq 0 ]; then
    rm -f "$@"
    return
  fi
  # a second per MiB it may have to send: xz manages about 4 MB/s on node1
  while read -r p size; do mib=$((mib + size / 1048576)); done < <(nix path-info -rs "${paths[@]}")
  if timeout -k 30 $((TIMEOUT + mib)) nix copy --to "$CACHE" "${paths[@]}"; then
    rm -f "$@"
  else
    echo "cache upload: ${#paths[@]} paths not sent (status $?), tried again later" >&2
    for f in "$@"; do failed "$f"; done
  fi
}

idle=0
while :; do
  now=$(date +%s)
  ready=() waiting=0
  while read -r t f; do
    if [ "${t%.*}" -gt "$now" ]; then
      waiting=1
    else
      ready+=("$f")
    fi
  done < <(listing)
  if [ "${#ready[@]}" -gt 0 ]; then
    idle=0
    # a file being retried goes alone, so one that cannot go holds back no other
    for f in "${ready[@]}"; do
      [ "$(tries_of "$f")" -gt 0 ] && send "$f"
    done
    fresh=()
    for f in "${ready[@]}"; do
      [ "$(tries_of "$f")" -eq 0 ] && fresh+=("$f")
    done
    for ((i = 0; i < ${#fresh[@]}; i += BATCH)); do
      send "${fresh[@]:i:BATCH}"
    done
  elif [ "$waiting" = 1 ]; then
    sleep 1
  elif [ "$idle" -lt "$IDLE" ]; then
    idle=$((idle + 1))
    sleep 1
  else
    exit 0
  fi
done
