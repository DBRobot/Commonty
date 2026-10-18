# Attestation test data

- `swtpm-vector.json`: one whole attestation by swtpm, made by
  `make-vector.sh` (swtpm 0.10.1, tpm2-tools 5.7). Its EK certificate is
  from a CA made for the run, under swtpm's vendor id; its event log is
  made up, and the PCRs were extended to match it. Its secrets are the
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
