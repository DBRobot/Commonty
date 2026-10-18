#!/usr/bin/env bash
# Makes swtpm-vector.json: one whole attestation by a software TPM, booted
# (as its made-up event log says) under Secure Boot with a db certificate
# of its own, for attest_test.go to judge again. Needs swtpm, tpm2-tools,
# openssl and go on PATH. Run from this directory.
set -euo pipefail
here=$PWD
w=$(mktemp -d)
trap 'kill "$(cat "$w/pid" 2>/dev/null)" 2>/dev/null || true; rm -rf "$w"' EXIT
swtpm socket --tpm2 --tpmstate dir="$w" --server type=unixio,path="$w/sock" \
  --ctrl type=unixio,path="$w/sock.ctrl" --flags not-need-init,startup-clear --pid file="$w/pid" --daemon
export TPM2TOOLS_TCTI="swtpm:path=$w/sock"
(cd ../../.. && go build -o "$w/attest-check" ./cmd/attest-check)
ac=$w/attest-check

# the maker: a test CA, its root under swtpm's vendor id
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=test-tpm-maker \
  -days 3650 -keyout "$w/ca.key" -out "$w/ca.pem" 2>/dev/null
openssl pkcs8 -topk8 -nocrypt -in "$w/ca.key" -out "$w/ca.p8"
tpm2_createek -Q -c "$w/ek.ctx" -G rsa -u "$w/ek.pub"
tpm2_flushcontext -t
$ac make-ek-cert -ek "$w/ek.pub" -ca-key "$w/ca.p8" -ca-cert "$w/ca.pem" -o "$w/ek.der"
tpm2_nvdefine -Q 0x1C00002 -C o -s "$(stat -c %s "$w/ek.der")" -a 'ownerread|ownerwrite|authread|authwrite'
tpm2_nvwrite -Q 0x1C00002 -C o -i "$w/ek.der"

# the boot: a db certificate, and the PCRs as the log says
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=test-box-db \
  -days 3650 -keyout /dev/null -out "$w/db.pem" 2>/dev/null
openssl x509 -in "$w/db.pem" -outform der -out "$w/db.der"
$ac make-log -db "$w/db.der" -o "$w/log.bin" | while IFS=: read -r pcr d; do
  tpm2_pcrextend "$pcr:sha256=$d"
done

# the join key, as disk-unlock-enrol makes it
mkdir -p "$w/keys"
tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
tpm2_startauthsession -Q -S "$w/s.ctx"
tpm2_policypcr -Q -S "$w/s.ctx" -l sha256:7 -L "$w/pcr.pol"
tpm2_flushcontext "$w/s.ctx"
tpm2_create -Q -C "$w/srk.ctx" -G ecc256:ecdsa-sha256 -L "$w/pcr.pol" \
  -a 'fixedtpm|fixedparent|sensitivedataorigin|sign|noda' -u "$w/keys/sig.pub" -r "$w/keys/sig.priv"
tpm2_flushcontext -t

mkdir -p "$w/roots/00001014"
cp "$w/ca.pem" "$w/roots/00001014/"
$ac run -roots "$w/roots" -keys "$w/keys" -dir "$w/ak" -log "$w/log.bin" -db "$w/db.der" \
  -software -vector "$here/swtpm-vector.json"
