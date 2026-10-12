# shellcheck shell=bash
# What a box asks of the DNS Worker for its own door, signed as the box:
#   door-ask tunnel <dir>   its own tunnel, made with a secret it makes here:
#                           <dir>/credentials.json for cloudflared
#   door-ask converge       the names put in step now: this box took a lease
# Environment: BOX, WORKER, KEYS, SOFT_KEY (box-sign.sh).
set -euo pipefail
w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
at=$(date +%s)
ask() { # <what> <extra json> <parts...>
  local what=$1 extra=$2
  shift 2
  { printf 'commonty %s v1\0%s\0%s' "$what" "$BOX" "$at"; for p in "$@"; do printf '\0%s' "$p"; done; } > "$w/msg"
  box_sign "$w/msg" "$w/sig.bin" || { echo "could not sign"; return 1; }
  sig=$(basenc --base64url -w0 < "$w/sig.bin" | tr -d =)
  # what it carries (a secret) through the environment, never arguments
  EXTRA=$extra jq -nc --arg box "$BOX" --arg at "$at" --arg sig "$sig" \
    '{box: $box, at: ($at | tonumber), sig: $sig} + (env.EXTRA | fromjson)' > "$w/body"
  curl -sS --max-time 60 -H 'content-type: application/json' --data-binary @"$w/body" "$WORKER/api/$what"
}
case "${1:-}" in
tunnel)
  dir=$2
  [ -s "$dir/credentials.json" ] && exit 0
  # its secret, made here: Cloudflare is told it to make the tunnel, and
  # this box keeps it; the Worker keeps nothing. Never in a command's
  # arguments, which anyone on the box may read: in the environment
  SECRET=$(openssl rand -base64 32)
  export SECRET
  got=$(ask tunnel "$(jq -nc '{secret: env.SECRET}')" "$SECRET")
  id=$(printf '%s' "$got" | jq -r '.id // empty')
  account=$(printf '%s' "$got" | jq -r '.account // empty')
  [ -n "$id" ] && [ -n "$account" ] || { echo "the Worker said: $got"; exit 1; }
  umask 077
  jq -nc --arg a "$account" --arg t "$id" \
    '{AccountTag: $a, TunnelSecret: env.SECRET, TunnelID: $t}' > "$dir/credentials.json.new"
  mv "$dir/credentials.json.new" "$dir/credentials.json"
  echo "this box's tunnel is $id"
  ;;
converge)
  got=$(ask converge '{}')
  printf '%s' "$got" | jq -e '.changed != null' >/dev/null || { echo "the Worker said: $got"; exit 1; }
  echo "names in step: $(printf '%s' "$got" | jq -r .changed) changed"
  ;;
*)
  echo "door-ask tunnel <dir> | converge"
  exit 1
  ;;
esac
