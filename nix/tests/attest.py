# A box proving it is real hardware, against swtpm (nix/tests/attest.nix)
box.start()
box.wait_for_unit("multi-user.target")
# securityfs says size 0; the firmware's log is there to read
box.succeed("test $(head -c 64 /sys/kernel/security/tpm0/binary_bios_measurements | wc -c) = 64")

# the join key, as dd-unlock-enrol makes it: under PCR 7's policy
box.succeed(
    "mkdir -p /tmp/keys && cd /tmp && "
    "tpm2_createprimary -Q -C o -G ecc256 -c srk.ctx && "
    "tpm2_startauthsession -Q -S s.ctx && "
    "tpm2_policypcr -Q -S s.ctx -l sha256:7 -L pcr.pol && "
    "tpm2_flushcontext s.ctx && "
    "tpm2_create -Q -C srk.ctx -G ecc256:ecdsa-sha256 -L pcr.pol "
    "-a 'fixedtpm|fixedparent|sensitivedataorigin|sign|noda' -u keys/sig.pub -r keys/sig.priv"
)

run = "attest-check run -roots /etc/attest-roots -keys /tmp/keys -dir /tmp/ak"

# a software TPM: refused unless the test switch says otherwise
out = box.fail(f"{run} -no-secure-boot 2>&1")
assert "software TPM" in out, out

# OVMF without keys enrolled: Secure Boot off, refused
out = box.fail(f"{run} -software 2>&1")
assert "Secure Boot is off" in out, out

# both switches: the whole exchange holds - the EK's chain, the AK beside
# it by two voters' credentials, the quote over their nonce against the
# firmware's log, the join key certified, its policy this boot's PCR 7
out = box.succeed(f"{run} -software -no-secure-boot 2>&1")
assert "attested j" in out and '"secureBoot":false' in out, out
print(out)
