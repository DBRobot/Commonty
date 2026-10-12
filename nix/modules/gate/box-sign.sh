# shellcheck shell=bash
# box_sign <message file> <signature file>: signed as this box, as
# acme-ask.sh and dd-control sign - with the TPM key dd-unlock-enrol made,
# usable only under this box's signed boot (PCR 7), or a test's PEM
# (SOFT_KEY). Environment: KEYS, SOFT_KEY.
box_sign() {
  if [ -n "${SOFT_KEY:-}" ]; then
    openssl dgst -sha256 -sign "$SOFT_KEY" -out "$2" "$1"
    return
  fi
  local w r
  w=$(mktemp -d)
  tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx" &&
    tpm2_load -Q -C "$w/srk.ctx" -u "$KEYS/sig.pub" -r "$KEYS/sig.priv" -c "$w/sig.ctx" &&
    tpm2_startauthsession -Q --policy-session -S "$w/s.ctx" &&
    tpm2_policypcr -Q -S "$w/s.ctx" -l sha256:7 &&
    tpm2_sign -Q -c "$w/sig.ctx" -g sha256 -s ecdsa -f plain -p "session:$w/s.ctx" -o "$2" "$1"
  r=$?
  tpm2_flushcontext "$w/s.ctx" >/dev/null 2>&1 || true
  tpm2_flushcontext -t >/dev/null 2>&1 || true
  rm -rf "$w"
  return $r
}
