#!/usr/bin/env bash
# Makes policy-vector.json: the join key's policy pieces as a TPM (swtpm)
# computes them, for policy_test.go to compute again. Needs swtpm,
# tpm2-tools, openssl and jq on PATH. Run from this directory.
set -euo pipefail
here=$PWD
w=$(mktemp -d)
trap 'kill "$(cat "$w/pid" 2>/dev/null)" 2>/dev/null || true; rm -rf "$w"' EXIT
swtpm socket --tpm2 --tpmstate dir="$w" --server type=unixio,path="$w/sock" \
  --ctrl type=unixio,path="$w/sock.ctrl" --flags not-need-init,startup-clear --pid file="$w/pid" --daemon
export TPM2TOOLS_TCTI="swtpm:path=$w/sock"
cd "$w"
hex() { od -An -v -tx1 "$1" | tr -d ' \n'; }
ft() { tpm2_flushcontext -t >/dev/null 2>&1 || true; }

openssl ecparam -name prime256v1 -genkey -noout -out a.key 2>/dev/null
openssl ec -in a.key -pubout -out a.pem 2>/dev/null
tpm2_loadexternal -Q -C o -G ecc -u a.pem -c a.ctx -n a.name
tpm2_readpublic -Q -c a.ctx -o a.tpm
ft
printf 'commonty boot v1' >ref
tpm2_startauthsession -Q -S t.ctx
tpm2_policyauthorize -Q -S t.ctx -L authorize.pol -q ref -n a.name
tpm2_flushcontext t.ctx

tpm2_nvdefine -Q 0x01500DD0 -C o -s 8 -a 'nt=counter|ownerwrite|ownerread|no_da'
tpm2_nvincrement -Q 0x01500DD0 -C o
tpm2_nvincrement -Q 0x01500DD0 -C o
tpm2_nvread -Q 0x01500DD0 -C o -o epoch.bin
tpm2_nvreadpublic 0x01500DD0 | sed -n 's/^ *name: *//p' | tr -d '\n' >epoch.name

head -c 32 /dev/urandom >pcr4
head -c 32 /dev/urandom >pcr7
cat pcr4 pcr7 >pcrs.bin
tpm2_startauthsession -Q -S t.ctx
tpm2_policypcr -Q -S t.ctx -l sha256:4,7 -f pcrs.bin -L pcr.pol
tpm2_policynv -Q -S t.ctx -i epoch.bin 0x01500DD0 eq -C o -L approved.pol
tpm2_flushcontext t.ctx

jq -n --arg approver "$(base64 -w0 a.tpm)" --arg approverName "$(hex a.name)" --arg authorize "$(hex authorize.pol)" \
  --arg epochName "$(cat epoch.name)" --arg epoch "$(hex epoch.bin)" --arg pcr4 "$(hex pcr4)" --arg pcr7 "$(hex pcr7)" \
  --arg pcrPolicy "$(hex pcr.pol)" --arg approved "$(hex approved.pol)" \
  '{approver: $approver, approverName: $approverName, authorize: $authorize, epochName: $epochName, epoch: $epoch,
    pcr4: $pcr4, pcr7: $pcr7, pcrPolicy: $pcrPolicy, approved: $approved}' >"$here/policy-vector.json"
