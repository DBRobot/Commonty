#!/usr/bin/env bash
# Makes swtpm-vector.json: one whole attestation by a software TPM, booted
# (as its made-up event log says) under Secure Boot with a db certificate
# of its own, through systemd-boot and a lanzaboote image of the release's
# stub, for attest_test.go to judge again. Needs swtpm, tpm2-tools,
# openssl, sbsigntools, binutils and go on PATH, and LOADER (systemd-bootx64.efi),
# STUB (lanzaboote_stub.efi) and KERNEL (a bzImage) set. Run from this
# directory.
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

# the boot: a db key of the box's own, systemd-boot and the box's
# lanzaboote image signed with it (sections added as lzbt adds them), and
# the PCRs as the log says
openssl req -x509 -newkey rsa:2048 -nodes -subj /CN=test-box-db \
  -days 3650 -keyout "$w/db.key" -out "$w/db.pem" 2>/dev/null
openssl x509 -in "$w/db.pem" -outform der -out "$w/db.der"
printf 'ID=nixos\nVERSION_ID=Generation 1, 2026-10-19\n' >"$w/osrel"
params='usbcore.autosuspend=-1 nohibernate root=fstab loglevel=4 lsm=landlock,yama,bpf'
printf 'init=/nix/store/0123456789abcdfghijklmnpqrsvwxyz-nixos-system-jbox-26.05/init %s' "$params" >"$w/cmdline"
printf '\\EFI\\nixos\\initrd.efi' >"$w/initrd"
printf '\\EFI\\nixos\\kernel.efi' >"$w/linux"
head -c 32 /dev/zero >"$w/initrdh"
openssl dgst -sha256 -binary "$KERNEL" >"$w/linuxh"
off=$(objdump -h "$STUB" | awk '$1 ~ /^[0-9]+$/ { s = $3; v = $4 } END { printf "%d", strtonum("0x" v) + strtonum("0x" s) }')
args=()
for sec in osrel cmdline initrd linux initrdh linuxh; do
  args+=(--add-section ".$sec=$w/$sec" --change-section-vma ".$sec=$(printf '0x%x' "$off")")
  off=$((off + $(stat -c %s "$w/$sec")))
done
objcopy "${args[@]}" "$STUB" "$w/image.efi"
sbsign --key "$w/db.key" --cert "$w/db.pem" --output "$w/image.signed.efi" "$w/image.efi"
sbsign --key "$w/db.key" --cert "$w/db.pem" --output "$w/loader.signed.efi" "$LOADER"
$ac boot -loader "$LOADER" -stub "$STUB" -kernel "$KERNEL" -params "$params" >"$w/boot.json"
$ac make-log -db "$w/db.der" -loader "$w/loader.signed.efi" -stub "$w/image.signed.efi" -o "$w/log.bin" | while IFS=: read -r pcr d; do
  tpm2_pcrextend "$pcr:sha256=$d"
done

# the join key: attest-check run makes it under its approver (dd-boot-key enrol)
mkdir -p "$w/keys"

mkdir -p "$w/roots/00001014"
cp "$w/ca.pem" "$w/roots/00001014/"
$ac run -roots "$w/roots" -keys "$w/keys" -dir "$w/ak" -log "$w/log.bin" -db "$w/db.der" \
  -stub "$w/image.signed.efi" -boot "$w/boot.json" \
  -software -vector "$here/swtpm-vector.json"
