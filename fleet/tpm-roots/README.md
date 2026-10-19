# TPM makers' roots

What a box's EK certificate must chain to before it may enlist
([docs/dev/attestation.md](../../docs/dev/attestation.md)). One folder per
maker, named by its TCG vendor id in hex, as an EK certificate's
alternative name gives it (`id:494E5443`); a certificate chains only to its
own maker's roots. The release puts them in the control plane's config
(`attest.roots`), so every validator judges by the same ones; `dd release
publish` refuses a file here that is not a CA certificate in date.

| folder | maker | file | fetched from | sha256 (first 16) |
|---|---|---|---|---|
| 494E5443 | Intel (PTT, On Die CA) | intel-ondie-ca-root.pem | https://tsci.intel.com/content/OnDieCA/certs/OnDie_CA_RootCA_Certificate.cer | BEB40BB7507B3396 |
| 414D4400 | AMD (fTPM) | amdtpm-root.pem | https://ftpm.amd.com/pki/aia/264D39A23CEB5D5B49D610044EEBD121 | 67BD2472A546751C |
| 49465800 | Infineon | optigarsarootca.pem | https://pki.infineon.com/OptigaRsaRootCA/OptigaRsaRootCA.crt | 899E35474C9807EB |
| 49465800 | Infineon | optigaeccrootca.pem | https://pki.infineon.com/OptigaEccRootCA/OptigaEccRootCA.crt | CFEB02FECD55AD7A |
| 49465800 | Infineon | optigarsarootca2.pem | https://pki.infineon.com/OptigaRsaRootCA2/OptigaRsaRootCA2.crt | B413C71580AF3EC5 |
| 49465800 | Infineon | optigaeccrootca2.pem | https://pki.infineon.com/OptigaEccRootCA2/OptigaEccRootCA2.crt | EE0C827555C4E901 |
| 53544D20 | STMicroelectronics | globalsign-tpm-root.pem | http://secure.globalsign.com/cacert/gstpmroot.crt | F27BF02C6E00C73D |
| 53544D20 | STMicroelectronics | stm-tpm-ek-root.pem | http://secure.globalsign.com/stmtpmekroot.crt | F8E37A86B689A20A |
| 53544D20 | STMicroelectronics | stm-tpm-ecc-root-01.pem | http://secure.globalsign.com/stmtpmeccroot01.crt | C541D7E5B9E3F935 |
| 4E544300 | Nuvoton | nuvoton-tpm-root-ca-1110.pem | https://www.nuvoton.com/security/NTC-TPM-EK-Cert/Nuvoton%20TPM%20Root%20CA%201110.cer | 2782E51A95E86D95 |
| 4E544300 | Nuvoton | nuvoton-tpm-root-ca-1111.pem | (same folder) ...%201111.cer | E38C280F7C0E0B49 |
| 4E544300 | Nuvoton | nuvoton-tpm-root-ca-2110.pem | (same folder) ...%202110.cer | 4AEBE77A51ED2995 |
| 4E544300 | Nuvoton | nuvoton-tpm-root-ca-2111.pem | (same folder) ...%202111.cer | CD8185FF8995ED09 |
| 4E544300 | Nuvoton | nuvoton-tpm-root-ca-2112.pem | (same folder) ...%202112.cer | 66E3A1013D4F6977 |

AMD's root (`CN=AMDTPM`, self-signed, 2014-2039) is where AMD's own EK
service keeps it, the address tpm2-tools' `tpm2_getekcertificate` uses for
AMD fTPMs; fetched again on 2026-10-19 with the same fingerprint. Second
source: David's laptop's AMD fTPM chain (EK -> `PRG-RPL` -> it) verifies to
it, by openssl and by the judge (`servers/system/control/state/attest/testdata`).

The two STM "root" files are STM's own CAs under GlobalSign's TPM root,
kept as anchors so a chain that stops at them still counts. GlobalSign's
TPM ECC root was not found at a stable link; STM's ECC CA stands in for it.

Fetched 2026-10-17. To add or replace one: fetch it from the maker's own
site over https where it has one, check its fingerprint against a second
source (the maker's documentation, or a machine's chain that verifies to
it), add the file and a row here, and release.
