# shellcheck shell=bash
# dd-sendmail: a sendmail for this box's services. The message on stdin, the
# recipients as arguments ("alerts", or a member's <name>@<domain>); -f and
# -F take a value and are ignored, as the Worker sets From itself. Handed to
# dd-mail (mail-send.sh) on its socket, which signs it with the TPM and
# gives it to the mail Worker. Exits 75 (try again later) when it was not
# sent, so a caller that retries does. From the wrapper: SOCK.
set -u
rcpts=()
skip=
for a in "$@"; do
  [ -n "$skip" ] && { skip=; continue; }
  case "$a" in
    -f | -F) skip=1 ;;
    -*) ;;
    *) rcpts+=("$a") ;;
  esac
done
[ ${#rcpts[@]} -gt 0 ] || { cat >/dev/null; exit 0; }

line="${rcpts[*]}"
reply=$({ printf '%s\n' "${line//$'\n'/ }"; cat; } | timeout 90 socat -t 60 - "UNIX-CONNECT:$SOCK" | tail -n 1)
case "$reply" in
  ok) exit 0 ;;
esac
echo "dd-sendmail: ${reply:-no answer from dd-mail}" >&2
exit 75
