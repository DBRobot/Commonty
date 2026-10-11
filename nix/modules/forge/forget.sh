#!/usr/bin/env bash
# Whoever left, or was removed 30 days ago (docs/dev/leaving.md): their own repositories and
# their account go. What they gave to someone else's repository stays there,
# under the forge's "Ghost" user.
set -euo pipefail
api=http://forgejo/api/v1
as_admin() { curl -fsS --unix-socket "$SOCK" -H "X-WEBAUTH-USER: $ADMIN" "$@"; }
list=$(curl -fsS "$GATE/internal/leaving") || { echo "the gate is not answering; next time"; exit 0; }
due=$(echo "$list" | jq -r '.members[] | select(.due and ((.done // []) | index("forge") | not)) | .name') || { echo "the gate's answer was not a list; next time"; exit 0; }
for name in $due; do
  case "$name" in *[!a-z0-9._-]* | [!a-z0-9]*) continue ;; esac
  id=$(echo "$list" | jq -r --arg n "$name" '.members[] | select(.name == $n) | .id')
  if [ "$name" = "$ADMIN" ]; then
    echo "member $id: the forge's admin; their account is kept until another is named"
    continue
  fi
  ok=1
  if as_admin "$api/users/$name" >/dev/null 2>&1; then
    for _ in $(seq 1 100); do
      repos=$(as_admin "$api/users/$name/repos?limit=50&page=1" | jq -r '.[].full_name') || { ok=0; break; }
      [ -n "$repos" ] || break
      for r in $repos; do
        case "$r" in "$name"/*) ;; *) continue ;; esac
        as_admin -X DELETE "$api/repos/$r" >/dev/null || { ok=0; break 2; }
      done
    done
    if [ $ok = 1 ] && ! as_admin -X DELETE "$api/admin/users/$name" >/dev/null; then
      echo "member $id: the forge refused to delete the account (the last owner of an organization?)"
      ok=0
    fi
    [ $ok = 1 ] && echo "member $id: their repositories and forge account are deleted"
  fi
  if [ $ok = 1 ]; then
    curl -fsS -X POST -H 'Content-Type: application/json' \
      -d "$(jq -cn --arg n "$name" '{name: $n, service: "forge"}')" "$GATE/internal/leaving/done" \
      || echo "member $id: the gate did not take the report"
  fi
done
exit 0
