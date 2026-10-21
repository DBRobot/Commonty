# shellcheck shell=bash
# dd-tpm-policy SESSION [KEYS]: satisfies the policy session SESSION for
# the box's join key (and what is sealed beside it) in KEYS (/boot/dd/unlock).
# A key under the box's approver (dd-boot-key, docs/dev/attestation.md):
# PCRs 4 and 7 now, the epoch now, and the approver's approval of them.
# A key made before that: PCR 7 alone.
set -euo pipefail
s=$1
keys=${2:-/boot/dd/unlock}
if [ ! -e "$keys/approver.tpm" ]; then
  tpm2_policypcr -Q -S "$s" -l sha256:7
  exit 0
fi
w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
tpm2_policypcr -Q -S "$s" -l sha256:4,7
tpm2_nvread 0x01500DD0 -C o -o "$w/epoch" 2>/dev/null
tpm2_policynv -Q -S "$s" -i "$w/epoch" 0x01500DD0 eq -C o -L "$w/cur.pol"
sig="$keys/approvals/$(od -An -v -tx1 "$w/cur.pol" | tr -d ' \n').sig"
if [ ! -e "$sig" ]; then
  echo "dd-tpm-policy: this boot is not one the box approved" >&2
  exit 1
fi
printf 'commonty boot v1' >"$w/ref"
cat "$w/cur.pol" "$w/ref" >"$w/msg"
tpm2_loadexternal -Q -C o -G ecc -u "$keys/approver.pem" -c "$w/approver.ctx"
tpm2_verifysignature -Q -c "$w/approver.ctx" -g sha256 -m "$w/msg" -s "$sig" -f ecdsa -t "$w/tk"
tpm2_policyauthorize -Q -S "$s" -i "$w/cur.pol" -q "$w/ref" -n "$keys/approver.name" -t "$w/tk"
