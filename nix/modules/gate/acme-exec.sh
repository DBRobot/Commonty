#!/bin/sh
# lego's exec provider: `present|cleanup <fqdn> <value>`, handed to the
# root helper that asks the DNS Worker (acme-ask.sh)
got=$(printf '%s %s %s\n' "$1" "$2" "$3" | socat -t 120 - UNIX-CONNECT:/run/dd-acme.sock)
[ "$got" = ok ] && exit 0
echo "acme challenge $1 $2: ${got:-no answer}" >&2
exit 1
