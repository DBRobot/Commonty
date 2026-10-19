# Attestation test data

- `swtpm-vector.json`: one whole attestation by swtpm, made by
  `make-vector.sh` (swtpm 0.10.1, tpm2-tools 5.8). Its EK certificate is
  from a CA made for the run, under swtpm's vendor id; its event log is
  made up, and the PCRs were extended to match it. It boots node1's
  systemd-boot (systemd 260.2) and an image of the release's lanzaboote
  stub (1.2.0) with lzbt's sections added by objcopy for node1's kernel
  (6.18.45), both signed with sbsign by the run's own RSA db key; its db
  holds that key and it logs no authority event. Its secrets are the
  run's own, random, and open nothing.
- `node1-ek-rsa.der`, `node1-ek-ecc.der`, `node1-ek-chain.bin`: node1's TPM
  (Intel PTT, Tiger Lake), read with `tpm2_nvread` from NV 0x01C00002,
  0x01C0000A and 0x01C00100 on 2026-10-17. Public certificates.
- `intel-tgl-00002004-ca.der`: from the AIA link in the chain's top
  certificate,
  https://tsci.intel.com/content/OnDieCA/certs/TGL_00002004_OnDie_CA.cer
- `intel-csme-intermediate.der`: from that one's AIA link,
  https://tsci.intel.com/content/OnDieCA/certs/OnDie_CA_CSME_Intermediate.cer
- `intel-ondie-root.der`: Intel's On Die CA root, the same as
  `fleet/tpm-roots/494E5443/intel-ondie-ca-root.pem`.
- `laptop-ek.pub`, `laptop-ek-rsa.der`: David's laptop (Lenovo Legion Pro 5
  16ADR10, AMD fTPM). The EK's TPM2B_PUBLIC from `tpm2_createek -G rsa` on
  2026-10-19, and its certificate from AMD's service at the link made from
  it, https://ftpm.amd.com/pki/aia/63b28c74805e8a4ed6d8cea39729f030
  (`tpm.AMDEKCertURL`). Public.
- `amd-prg-rpl-ica.der`: from that certificate's AIA link,
  http://ftpm.amd.com/pki/aia/9DE34E7EF9C15FA479A9F398BC865D60
- `amd-root.der`: AMD's root, the same as
  `fleet/tpm-roots/414D4400/amdtpm-root.pem`.
- `ms-uefi-ca-2011.der`, `canonical-master-ca.der`: from the laptop's
  firmware event log (its db, and shim's MokListRT): shared Secure Boot
  certificates a box may not claim as its own.
