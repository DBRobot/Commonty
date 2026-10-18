# Attestation: proof of real hardware

Status: built on branch `tpmattest-1017`; nothing calls it yet. The
enlistment (branch `joining-1017`) opens an attestation when a member's box
asks to enlist, and counts the box only once it has attested.

A member's box earns its owner access by being real hardware: a TPM 2.0
chip of its own, one box per chip, booted under Secure Boot with the box's
own keys (decided 2026-10-17). This page is how a box proves that, what the
proof is worth, and what it is not.

## What it proves

- **A TPM a maker made.** The TPM's endorsement key (EK) certificate chains
  to that maker's root, shipped in the release (`fleet/tpm-roots`), checked
  at the agreed time. A software TPM (swtpm, the IBM simulator) is refused
  unless the release's config says `attest.test.softwareTPM`, which only
  tests set.
- **One box per TPM.** The agreed state keeps the hash of each EK's public
  key against the box that proved with it. Only the RSA 2048 EK of the
  TCG's default template counts (every PC TPM has its certificate): a TPM
  with an ECC EK as well can't be two boxes. A second box with the same TPM
  is refused until the first is gone (`Forget`, when its enlistment ends).
- **The box's key lives in that TPM.** The box makes an attestation key
  (AK) under the EK. Each box voter seals a fresh secret to the EK for the
  AK's name (credential activation, TPM2_MakeCredential): only that TPM,
  holding that AK, opens it. The AK then certifies the box's join key
  (TPM2_Certify): the join key, fixed to its TPM, is in the AK's TPM.
- **What booted, now.** The AK quotes PCRs 0, 2, 4 and 7 over a nonce
  made of every voter's secret. The firmware's event log, replayed, must
  give the quoted PCRs; then its PCR 7 events must say Secure Boot was on
  and that the box's own certificate (its installer's record) let what
  booted run, in db with keys of its own or in MOK under a Microsoft-signed
  shim. The join key's policy must be PCR 7 as quoted: the key signs
  nothing under any other boot. The record keeps all four PCRs.

## What it does not prove

- That the machine is the one the member says, or theirs, or at their house.
- That what booted is our release: PCR 4 is recorded, not judged (a
  release changes it). PCR 7 says only that the box's own key let it run.
- Anything after the boot it was made in. The quote is one moment; the
  join key's PCR 7 policy is what keeps it to that boot afterwards.
- Against an attacker with the chip on a bench (a probe on the TPM's bus,
  a fault on the CPU running a firmware TPM). A maker's key leaking, or a
  TPM whose EK certificate was issued wrong, passes.
- That the box keeps its promises: placement and the stores watch that.

## The flow

```
enlisting box       Begin rides in its request to enlist:
                      EK public, EK certificate + chain, AK public,
                      join key public, event log, its own Secure Boot cert
enlistment judge    CheckAttestOpen / AttestOpen: chain to a maker's root,
                      shapes of EK/AK/join key, join key = the box's key,
                      EK not another box's -> Attest.Pending[box]
each box voter      attest-challenge: {box, validator, cred, secret,
                      commit = sha256(secret)}, once per attestation
enlisting box       when a majority of box voters have challenged:
                      tpm2_activatecredential each, nonce =
                      sha256("commonty attest v1" box, then each voter and
                      its secret, in voter order), tpm2_quote over it,
                      tpm2_certify of the join key; attest-response, signed
                      by the join key
every validator     the secrets match their commits, the quote and the
                      certification verify -> Attest.Done[box], EKs[hash]
```

One honest voter is enough. Its secret is only in its sealed challenge, so
no answer that holds it could have been made earlier, by another TPM, or
without the AK. A majority have to challenge, so the challenges include an
honest voter wherever most are honest. An attestation not answered within
an hour is dropped; the box starts again. The judge is deterministic, like
the rest of the state: no clock, no network, no randomness.

The certification carries no nonce (tpm2_certify takes none). It needs
none: the AK is shown fresh and in the EK's TPM by the activation, and a
key fixed to its TPM that this AK certified at any time lives there.

## Where it is

| | |
|---|---|
| judge | `servers/system/control/state/attest` (`Begin`, `Challenge`, `Response`, `State`); `state.go` kinds `attest-challenge`, `attest-response` |
| enlistment's calls | `State.CheckAttestOpen`, `State.AttestOpen`, `State.Attested(box)`, `attest.State.Forget` |
| box side | `servers/system/control/tpm` (`Box.Begin`, `Box.Respond`); `dd-control -attest-begin` prints Begin |
| voter side | `tpm.MakeChallenge`; dd-control's loop (`cmd/dd-control/attest.go`) challenges every box attesting, and answers for its own box |
| roots | `fleet/tpm-roots/<vendor id>/*.pem` -> config `attest.roots` |
| tests | `state/attest` unit tests on a swtpm exchange and node1's real Intel chain; `nix/tests/attest.nix` (swtpm, OVMF's real event log); `dd` `tpm_roots` tests |

A box's AK files are in `/var/lib/dd-control/attest`; the AK is loaded
under the EK again for each answer. The join key is the one
`dd-unlock-enrol` makes (`/boot/dd/unlock/sig.pub`).

## Each maker's chain

The judge is offline; the box gathers the chain. It reads the EK
certificate from NV (0x01C00002, the RSA one), the TPM's own
chain from NV 0x01C00100 where it has one, then follows each top
certificate's AIA link until a self-signed one (left out: the release has
the roots).

- **Intel PTT** (node1, Tiger Lake): EK -> three on-die certificates in
  0x01C00100 -> the platform's On Die CA issuing CA -> Intel's CSME
  intermediate (both by AIA from tsci.intel.com) -> Intel's On Die CA
  root. Verified with node1's real certificates.
- **Infineon, STMicroelectronics, Nuvoton** (discrete TPMs): EK in NV ->
  the maker's manufacturing CA by AIA -> root. Not yet seen on a real box.
- **AMD fTPM** (node2): **no EK certificate in NV**, and AMD's service
  (ftpm.amd.com/pki/aia/...) did not answer for node2's EK under the
  naming we tried. Such a box cannot attest yet (`Begin` says so). Options:
  find AMD's naming (tpm2-tools' `tpm2_getekcertificate` knows it) and
  fetch on the box; or pin AMD EKs one by one; or refuse AMD fTPMs.

## Updating the roots

The roots are files in `fleet/tpm-roots`, one folder per maker by TCG
vendor id, listed with where each came from and its fingerprint in its
README. To add or replace one: fetch it from the maker's own site, check
its fingerprint against a second source, add the file and a README row,
and release. `dd release publish` refuses the release if any file there is
not a CA certificate in date or not under a vendor id. Every validator
judges by the roots of the release it runs, as with any other rule.
