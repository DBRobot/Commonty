# Attestation: proof of real hardware

Status: wired into joining. A box's join carries its TPM's proof (Begin);
the box is joined only once its answer to the box voters' challenges
holds, and a refusal is kept with why, for the box and its member.

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
  give the quoted PCRs. Then:
  - **PCR 7**: Secure Boot on, a platform key (not setup mode), and the
    box's own certificate (its installer's record, sent in Begin) in the
    db measured. Where the firmware logs which db entry let a boot
    application run (EV_EFI_VARIABLE_AUTHORITY), one of them must be the
    box's; many firmwares log none (David's laptop did not), and its
    absence is no refusal.
  - **PCR 4**: the boot applications the firmware started, by Authenticode
    digest, are exactly two: one of the release's systemd-boot builds,
    then the box's lanzaboote image. The box sends that image (the
    ESP's `EFI/Linux` entry systemd-boot chose, `LoaderEntrySelected`);
    its digest must be the second. It must be the release's stub - the
    same headers, data directories and entry point (in the stub's own
    code), and every section in order at its address with its contents -
    followed by exactly lzbt's six sections in lzbt's order, each data
    (never code or writable), past the stub's own addresses and inside the
    image the firmware loads. Its `.linuxh` must be one of the release's
    kernels, and its `.cmdline` `init=` a NixOS system's followed by one
    of the release's kernel parameter lines. Its `.initrdh` is judged
    only against a list the release gives; it gives none, as a box's
    initrd is its own build (its hardware), which a release cannot list
    for a box it has not seen. One parse of the image serves both the
    digest and these checks. A shim, a grub, Windows' boot manager or anything
    else in PCR 4, or any other kind of PCR 4 event (an edited command
    line), and the boot is not ours. Separators and the profile's
    EV_EFI_ACTION strings pass only with a digest of their own data;
    PCR 7 events other than variables, separators and no-action ones are
    refused. The Authenticode digest leaves out the
    signature, so it is the same whichever box's key signed the image.
  - The join key's policy must be PolicyAuthorize by the box's approver
    (below), and this boot must be one the approver approved at the epoch
    the TPM certifies now (TPM2_NV_Certify by the AK over the nonce). The
    record keeps all four PCRs.
- **One box per Secure Boot key.** The box's db certificate must be its
  own: Microsoft's (any it made or signed) and Canonical's shim key are
  refused, and the agreed state keeps the hash of each box's, so no two
  boxes claim one.

## What it does not prove

- That the machine is the one the member says, or theirs, or at their house.
- That the box's own system is unchanged past its stub: which system's
  init the command line names, and the initrd, are the box's own build,
  not checked against the release's; the kernel and kernel parameters are.
- Anything after the boot it was made in. The quote is one moment; the
  join key's policy is what keeps it to approved boots afterwards (below),
  and an approved boot whose system was taken over can approve others.
  dbx is not judged (a box whose keys were cleared may have none; PCR 4
  refuses what it would at the attested boot).
- Against an attacker with the chip on a bench (a probe on the TPM's bus,
  a fault on the CPU running a firmware TPM). A maker's key leaking, or a
  TPM whose EK certificate was issued wrong, passes.
- That the box keeps its promises: placement and the stores watch that.

## The flow

```
joining box         Begin rides in its join (JoinReq.Attest):
                      EK public, EK certificate + chain, AK public,
                      join key public, event log, its own Secure Boot cert,
                      the lanzaboote image it booted
every validator     state.JoinRefusal: the hardware minimum, then
                      CheckAttestOpen (chain to a maker's root, shapes of
                      EK/AK/join key, join key = the box's key, EK and db
                      certificate not another box's) and JudgeBoot (PCR 7:
                      Secure Boot on with its own certificate; PCR 4: the
                      release's systemd-boot and stub, from the log)
                      -> Joining[box], Attest.Pending[box]
each box voter      attest-challenge: {box, validator, cred, secret,
                      commit = sha256(secret)}, once per attestation
enlisting box       when a majority of box voters have challenged:
                      tpm2_activatecredential each, nonce =
                      sha256("commonty attest v1" box, then each voter and
                      its secret, in voter order), tpm2_quote over it,
                      tpm2_certify of the join key; attest-response, signed
                      by the join key
every validator     the secrets match their commits, the quote and the
                      certification verify -> Attest.Done[box], EKs[hash],
                      and the box is Joined (its `joined` record, with its
                      member's ticket, in the directory log)
```

A join refused for what the box is - below the minimum, its TPM, its
boot - and an answer that does not hold are kept in the agreed state with
why (`Refused`, a week; the same one again only after a day; 64 at once,
a ticketless one given up first), in the person's words (`attest.Why`): "its TPM is
not one a TPM maker certified", "Secure Boot is off", "it did not start
the fleet's own system", "it needs 8 GiB of memory (it has 3.8 GiB)". Where
the join carried a member's ticket, the identity judge is told too
(`{"refused": ...}`, written by the state machine alone) and shows it to
that member only (`View.Refused`, their Devices page). A join with no
answer within the hour is refused as such. The box says the same before
it asks, by the same function (`state.JoinRefusal`, dd-control's
`join-refused`). A joined box that has not proved its TPM (a record lost)
counts for no one's access (`BoxInfo.Unattested`).

Bounds, so junk cannot crowd out a member's refusal or be replayed: a
join's Seq is its sender's clock and must be within ten minutes of the
agreed time and above any Seq of that box already judged (`JoinFresh`;
a refusal keeps it), so an old signed join replayed is stale before it is
judged. A refusal counts as a member's only when the identity state the
directory log holds says its ticket is theirs: signed by a current device
of the member it names, who is not banned, and unused
(`ident.TicketOwner`, read from the log's replay). Those have their own
bucket (16 an hour, one a ticket and member) and go to the identity judge
to be shown to that member. Every other refusal, a made-up ticket's
included, is a stranger's: its own bucket (4 an hour), never in the
directory log, and the first given up when too many are kept.

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
| the join's calls | `state.JoinRefusal`, `State.CheckAttestOpen`, `State.AttestOpen`, `State.Attested(box)`, `attest.State.Forget` (a joined box gone); `state/join.go` `Joining`, `Refused` |
| box side | `servers/system/control/tpm` (`Box.Begin`, `Box.Respond`); `dd-control -attest-begin` prints Begin |
| voter side | `tpm.MakeChallenge`; dd-control's loop (`cmd/dd-control/attest.go`) challenges every box attesting, and answers for its own box |
| roots | `fleet/tpm-roots/<vendor id>/*.pem` -> config `attest.roots` |
| join key across updates | `state/attest/policy.go`; `tpm/bootkey.go`; `cmd/boot-key` (`dd-boot-key`); `nix/modules/box/tpm-policy.sh` (`dd-tpm-policy`); `nix/modules/box/boot-key.nix` |
| tests | `state/attest` unit tests on a swtpm exchange, node1's real Intel chain, the laptop's real AMD chain and the policies as swtpm computed them; `state` tests of the join through the agreed state (joined on its proof; refused for its EK, a software TPM, PCR 4, the minimum, no proof, a proof that does not hold, no answer); `nix/tests/attest.nix` (swtpm, OVMF's real event log); `nix/tests/boot-key.nix` (an update across a boot-image change, a rollback, a boot never approved); `nix/tests/join-attest.nix` (below); `dd` `tpm_roots` tests |
| real hardware | `attest-check inspect -roots fleet/tpm-roots`: the EK's chain, the log replayed against the TPM's PCRs, Secure Boot, PCR 4's applications; changes nothing in the TPM |

A box's AK files are in `/var/lib/dd-control/attest`; the AK is loaded
under the EK again for each answer. The join key is the one
`dd-boot-key enrol` makes (`/boot/dd/unlock/sig.pub`).

## The join key across updates

Decided 2026-10-19 (David): the join key is bound to PCR 4 as well as
PCR 7, so a boot of any other loader the firmware's db lets run - a
Microsoft-signed one, on a box enrolled with Microsoft's keys beside its
own - gets no key, even where the firmware logs no authority events. PCR 4
changes with every release's boot image, so the binding goes through an
approver:

- **The policy.** The join key (and what joined boxes seal beside it) has
  the policy TPM2_PolicyAuthorize(approver A, "commonty boot v1"). A
  session satisfies it only with an approval: A's signature over
  PolicyPCR(4, 7 as this boot measured) then PolicyNV(the box's epoch
  counter, 0x01500DD0, equal to the epoch the approval names).
  `dd-tpm-policy` makes that session for every user of the key (the
  initrd's disk unlock, dd-control, the gate's, mail's and ACME's
  signatures); a key made before (node1, node2) keeps PCR 7 alone until
  it is enrolled again.
- **The approver.** A P-256 key made at enrolment; its public half beside
  the join key, its private half sealed in the TPM under the same
  PolicyAuthorize. So only a boot already approved can open it to approve
  another; a boot never approved has neither the join key nor A.
- **An update.** After a switch, lzbt has put the new generation's image
  on the ESP; the activation step (`dd-boot-key approve-esp`) predicts
  each of the newest three generations' PCR 4 - this boot's log, with
  systemd-boot's and the image's Authenticode digests in place of the ones
  that ran - and approves them at the current epoch, from the boot that is
  running. The running boot stays approved: there is no moment when
  neither the old nor the new boot opens the key.
- **Settling.** At each start, `dd-boot-settle`: if this boot came up
  approved and is new, it joins the good boots (the last three, newest
  first); each is approved for the next epoch (and again with this boot's
  systemd-boot, which lzbt may have replaced); then the counter is raised,
  which retires every approval of the epoch before, and their files go.
  So a rollback to any of the last three good boots opens the key, and an
  approval for anything older - an old release, a file someone copied off
  the boot partition - no longer does.
- **The counter** is a TPM NV counter: it only goes up, and one defined
  again (the owner can delete it) starts at the highest value any counter
  in the TPM reached, so the epoch cannot be set back to revive an old
  approval. Deleting it only stops the key opening.
- **The judge.** A re-attestation after a switch quotes the new PCRs and
  judges them as at the first (PCR 4 against the release's boot images);
  it also checks the approval of this boot at the certified epoch.

**What this cannot stop: an approved boot taken over.** Root on a boot
that is approved can unseal A and sign approvals for any PCR values at any
epoch, so the key would open under a boot of its choosing from then on -
until the box enrols again. Every self-approving scheme has this (BitLocker
and systemd-pcrlock alike): the TPM cannot know what a release is. The
variant here is the narrowest we found:

- A is sealed under its own approvals, never on disk in the clear, and
  opens only in an approved boot: a box booted any other way (another
  loader, a USB system, a Microsoft-signed shim) cannot approve itself.
- Not TPM2_PolicyAuthorizeNV: the NV index holding the approved policy
  would be the owner's to delete and define again with a password of the
  attacker's own (the owner's authorization is empty on our boxes, and an
  index's name leaves its password out), from any boot. A signature by A
  has no such way round.
- The epoch retires every approval A made honestly, but not ones a
  taken-over boot made for epochs to come.
- `dd-boot-key` approves only what it predicts from this boot's own log
  and the images on the ESP, and the judge refuses a re-attestation whose
  PCR 4 is not the release's.

### On a joined box

Its first own-keys boot (`dd-unlock-seal`) makes the join key with
`dd-boot-key enrol` first, then seals its disk half under the same policy
(`/boot/dd/unlock/join.policy`), opened with `dd-tpm-policy` in the
initrd; `dd-join-prepare` does the same where the seal did not.
`nix/modules/box/boot-key.nix` is in every box's modules. The release's
PCR 4 list (`attestBoot`) has a joined box's kernel parameters beside the
release's boxes': a joined box is built on itself, so the release builds
one from a stand-in report, with and without a USB network link's
parameter.

## How it is tested end to end

`join-attest` (VM): OVMF with Secure Boot under keys of the box's own,
lanzaboote's signed image, swtpm with an EK certificate from a test maker,
the join key made by `dd-boot-key enrol`; the join through the agreed
state's own judgement (`attest-check join`: `state.Block`, two box voters'
challenges, the TPM's answer). A good box joins. An EK from a CA not its
maker's, and a software TPM under the release's real roots, are refused
("its TPM is not one a TPM maker certified"); a release whose PCR 4 list
is not this boot's refuses it ("it did not start the fleet's own
system"). An update: the box restarts (dd-boot-settle approves what its
ESP starts), then starts the next image (another command line): its join
key opens there with no one doing anything, it is refused by the release
that did not list that boot and joins under the one that does. Secure
Boot off (keys cleared): the key opens nothing and the box says "Secure
Boot is off".

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
- **AMD fTPM** (node2, David's laptop): no EK certificate in NV. AMD
  serves each one at `https://ftpm.amd.com/pki/aia/<id>`, where `<id>` is
  the first 16 bytes, hex, of sha256(`00 00 22 22` || the EK's exponent, 4
  bytes big-endian || its modulus) - tpm2-tools' `tpm2_getekcertificate`
  scheme (`tpm.AMDEKCertURL`). The box reads the maker from `tpm2_getcap
  properties-fixed` and, for AMD, fetches its certificate at Begin; the
  AIA links take it up to AMD's root (`AMDTPM`). The judge does not fetch:
  the certificate rides in Begin like any other. Verified with David's
  laptop (Legion Pro 5, 2026-10-19): EK -> `PRG-RPL` (AMD, Santa Clara;
  by AIA from ftpm.amd.com) -> `AMDTPM`. Its certificate is a plain
  rsaEncryption key with a positive serial, the EK purpose 2.23.133.8.1,
  AMD's own policy 1.3.6.1.4.1.3704.101.21.10, an empty subject, and the
  TCG alternative name (id:414D4400, model AMD). An EK certificate whose
  key is named RSAES-OAEP is read as the RSA key it is. One that names no
  maker counts only as AMD's: issued in AMD's name and chained to AMD's
  roots, never tried against other makers'. Every EK certificate must carry
  the TCG's EK purpose (2.23.133.8.1) or a TCG certificate policy.

A real event log (the laptop's, AMD, Lenovo firmware, 127 events)
replays to all eight live PCRs, and the Authenticode digests computed here
of its shim and grub match what its firmware put in PCR 4. Its PCR 7 has
no db authority event: hence the PCR 4 rule.

## Where the release's boot comes from

`control.configFile` (flake.nix) adds `attest.boot` from `attestBoot`:
for each box configuration that boots with lanzaboote, `attest-check boot`
reads its systemd-boot (`systemd/lib/systemd/boot/efi/systemd-bootx64.efi`,
which lzbt signs onto the ESP), lanzaboote's stub
(`lanzaboote.packages.<system>.stub`, the one lzbt adds sections to) and
its kernel file, and the lists are merged. lzbt keeps the ESP's systemd-boot
until a newer one comes, so a box still on an older systemd-boot than the
release's does not match until it has one; a release may carry earlier
releases' digests too (not done yet).

## Secure Boot on a member's box

Decided 2026-10-19: a member's box boots with Secure Boot on. Its
installer (branch `joining-1017`, `nix/installer/join-install.sh`) does it
with keys of the box's own, as node1 and node2 have:

1. The person puts Secure Boot in **setup mode** (its keys cleared;
   Secure Boot itself stays enabled). Where Linux can, the installer's
   firmware step does it (`bios-settings.json`: ThinkPad/ThinkCentre
   `thinklmi`, HP `hp-bioscfg`) and restarts; elsewhere the screen says
   where:
   - Lenovo consumer (Legion, IdeaPad, Yoga): F2 at power-on, Security ->
     Secure Boot -> Reset to Setup Mode (or Clear All Secure Boot Keys),
     F10.
   - Dell: F2, Secure Boot -> Expert Key Management -> Custom Mode on ->
     Delete All Keys, Apply.
   - HP: F10, Advanced -> Secure Boot Configuration -> Clear Secure Boot
     Keys; HP asks for a code shown on screen at the next start.

   None needs a BIOS password: Windows-certified PCs let the person at the
   firmware screen clear the keys. Not yet tried on every model.
2. The installer stick starts (setup mode checks no signatures), installs,
   makes the box's keys on its encrypted disk (`/var/lib/sbctl`), signs the
   boot with lanzaboote, and enrolls them with Microsoft's beside them
   (graphics and network cards' option ROMs are Microsoft-signed). Enrolling
   the platform key ends setup mode: from the next start Secure Boot runs
   only what the box's key signed.
3. The first boot is under the box's own db key, and the join key is made
   then, under its approver (`dd-boot-key enrol`, see below). Attestation checks that PCR 7
   (Secure Boot on, a PK, the box's `db.pem` in db) and PCR 4 (the
   release's systemd-boot and stub). Nothing to confirm at that boot.

Not shim + MOK: NixOS has no Microsoft-signed shim, a borrowed distro shim
is at the mercy of SBAT revocations, Microsoft's 2011 UEFI CA (which signed
the shims older firmware trusts) expired in June 2026 while firmware not
updated by Windows lacks the 2023 one, many new Lenovo and Dell machines
ship with the third-party CA turned off, and MOK needs a password typed at a
blue screen. A box booted through shim is refused by the PCR 4 rule.

What the installer must keep for attestation (it does, on `joining-1017`):
the db certificate at `/var/lib/sbctl/keys/db/db.pem` (what `tpm.Default`
sends in Begin), the join key made only at a boot with Secure Boot on, and
no seal and no join key while Secure Boot is off or still in setup mode
(a firmware whose own Secure Boot switch was off): `dd-unlock-seal` waits,
says why, and seals at the first start with it on.

## Updating the roots

The roots are files in `fleet/tpm-roots`, one folder per maker by TCG
vendor id, listed with where each came from and its fingerprint in its
README. To add or replace one: fetch it from the maker's own site, check
its fingerprint against a second source, add the file and a README row,
and release. `dd release publish` refuses the release if any file there is
not a CA certificate in date or not under a vendor id. Every validator
judges by the roots of the release it runs, as with any other rule.
