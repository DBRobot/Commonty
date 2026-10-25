# shellcheck shell=bash
# One challenge, asked of the DNS Worker for lego (acme-exec.sh), over a
# socket only the acme user may open. lego never holds the box's key: this
# runs as root, signs with the TPM, and asks only for a name this box
# serves. Line in: "present|cleanup <fqdn> <value>"; line out: "ok" or why not.
# Environment: BOX, WORKER, NAMES (one per line), KEYS, SOFT_KEY (a test's
# PEM in place of the TPM).
set -u
read -r op fqdn value || exit 1
no() { echo "$*"; exit 1; }
case "$op" in present | cleanup) ;; *) no "present or cleanup" ;; esac
name=${fqdn%.}
name=${name#_acme-challenge.}
[ "_acme-challenge.$name" = "${fqdn%.}" ] || no "not a challenge name: $fqdn"
printf '%s\n' "$NAMES" | grep -qxF -- "$name" || no "not a name this box serves: $name"
case "$value" in *[!A-Za-z0-9_-]*) no "not a challenge" ;; esac

w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
at=$(date +%s)
printf 'commonty acme v1\0%s\0%s\0%s\0%s\0%s' "$BOX" "$at" "$op" "$name" "$value" > "$w/msg"
if [ -n "${SOFT_KEY:-}" ]; then
  openssl dgst -sha256 -sign "$SOFT_KEY" -out "$w/sig.bin" "$w/msg" || no "could not sign"
else
  # as dd-control signs (servers/system/control/cmd/dd-control): the key
  # dd-unlock-enrol made, usable only under this box's signed boot (PCR 7);
  # one user of the TPM at a time on the box (tpmsign.go)
  lock=/run/dd-tpm/lock
  # root's alone (box/control.nix); without it, a lock of its own
  if [ ! -r "$lock" ]; then
    lock="$w/lock"
    : >"$lock"
  fi
  {
    flock -w 10 9 || true
    tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx" &&
      tpm2_load -Q -C "$w/srk.ctx" -u "$KEYS/sig.pub" -r "$KEYS/sig.priv" -c "$w/sig.ctx" &&
      tpm2_startauthsession -Q --policy-session -S "$w/s.ctx" &&
      tpm2_policypcr -Q -S "$w/s.ctx" -l sha256:7 &&
      tpm2_sign -Q -c "$w/sig.ctx" -g sha256 -s ecdsa -f plain -p "session:$w/s.ctx" -o "$w/sig.bin" "$w/msg"
    r=$?
    tpm2_flushcontext "$w/s.ctx" >/dev/null 2>&1 || true
    tpm2_flushcontext -t >/dev/null 2>&1 || true
  } 9<"$lock"
  [ $r -eq 0 ] || no "the TPM did not sign"
fi
sig=$(basenc --base64url -w0 < "$w/sig.bin" | tr -d =)

jq -nc --arg box "$BOX" --arg at "$at" --arg op "$op" --arg name "$name" --arg value "$value" --arg sig "$sig" \
  '{box: $box, at: ($at | tonumber), op: $op, name: $name, value: $value, sig: $sig}' > "$w/body"
got=$(curl -sS --max-time 60 -H 'content-type: application/json' --data-binary @"$w/body" "$WORKER/api/challenge" 2>&1)
[ "$(printf '%s' "$got" | jq -r '.ok // empty' 2>/dev/null)" = true ] || no "the Worker said: $got"
echo ok
