# shellcheck shell=bash
# Museum's login route dies from time to time: its rate limiter's map hits
# a Go runtime panic (ulule/limiter, "ran out of hash bits") and every
# request through it is a 500 until the process restarts. Asked for an
# address nobody has, a healthy museum says 404; a broken one says 500.
# not listening yet (a box starting) is curl's failure, 000: left alone
code=$(curl -s -o /dev/null -m 10 -w '%{http_code}' "http://127.0.0.1:$PORT/users/srp/attributes?email=nobody%40health.invalid") || true
case "$code" in
  404) exit 0 ;;
  500) echo "museum answers 500 on its login route; restarting it"; systemctl restart ente.service ;;
  *) echo "museum answered $code; leaving it" ;;
esac
