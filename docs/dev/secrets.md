# Secrets and keys

`secrets/` holds the fleet's secrets, encrypted with
[sops](https://github.com/getsops/sops) to age keys and committed. The
repository is public; the files are safe to publish because every value in
them is ciphertext. Each box can read its own file and no other. This page
is how the files are laid out, who can decrypt what, how a box and a module
read a secret, and how to add or change one.

## The layout

```
.sops.yaml            the recipients, and which file is encrypted to whom
secrets/
  node1.yaml          node1's secrets: every service secret it runs
  node2.yaml          node2's secrets
  fleet.yaml          the private half of the box list, the cache writer's
                      keys, the console password; no box can read it
```

Only key names and structure are in this page. Values are never written
anywhere but the encrypted files.

## Who can decrypt what

[`.sops.yaml`](../../.sops.yaml) names the recipients and gives one
creation rule per file:

```
secrets/node1.yaml   node1-tpm, david, david-dd
secrets/node2.yaml   node2-tpm, david, david-dd
secrets/fleet.yaml   david-dd, david
```

- **`<box>-tpm`** is the box's secrets key: an age key made inside its TPM
  by `age-plugin-tpm`, once, by `dd-secrets-key.service`
  ([`nix/modules/box/report.nix`](../../nix/modules/box/report.nix)). The
  file `/var/lib/dd/secrets-key.txt` on the box is a handle only that TPM
  can use; a copy of it, or of the disk, opens nothing. Its public half is
  `secretsKey` in the box's signed `fleet/boxes/<box>/hardware.json`, and
  that is where the recipient in `.sops.yaml` comes from.
- **`david-dd`** is the editing key `dd secret init` made, held in the
  laptop's credential store. `dd secret run` hands it to sops.
- **`david`** is an older key derived from David's ssh key. It stays as a
  recipient, but its ssh key has a passphrase sops cannot use.
- **`node1`, `node2`** (the ssh-derived ones at the top of `.sops.yaml`)
  are no longer recipients of anything. They stay as names in the file's
  history.

No box is a recipient of another box's file, and no box is a recipient of
`fleet.yaml`. A box that sits in someone else's house one day must not be
able to read what another box runs.

## What is in each file

`secrets/<box>.yaml` is a flat map, one key per secret. The names, and the
bundle that reads each:

```
every box   wifi-psk                     host.nix: the house Wi-Fi
            console-password-hash        core: admin's password at the keyboard
storage     garage-rpc-secret            garage's cluster secret, shared by storage boxes
            garage-backup-key-id/-secret this box's key to its own backup bucket
            restic-password              this box's restic repository password
            thanos-key-id/-secret        the metrics bucket, for the sidecar
            cache-key-id/-secret         read key to the nix-cache bucket (also runner)
runner      nix-cache-signing-key        no longer read: the box makes its own (below)
gateway     cloudflare-token             the zone's names (dd-dns) and the ACME challenge,
                                         only until `dd dns setup`: then unused,
                                         deleted at Cloudflare and from this file
            cloudflared-credentials      the tunnel
            cloudflare-workers-read      the mail and DNS Worker checks (when configured)
            tmdb-token                   the gate's Movies & TV lookups
photos      ente-key-encryption, ente-key-hash, ente-jwt-secret, ente-ott,
            ente-demo-password,
            garage-key-id/-secret        ente's own bucket key
library     library-key-id/-secret       the libraries bucket
vault       vault-key-id/-secret, vault-oidc-secret
adblock     pihole-api-password          no longer read: the box makes its own (below)
```

node1's file has every key above. node2's has core's, storage's and the
runners' only, so node2 is built only with the bundles those cover
(below).

The runners' registration tokens are in no box's file. The forge's box
makes them and seals each to its runner box's secrets key
([`runner-keys.nix`](../../nix/modules/forge/runner-keys.nix)); the
runner box fetches its own and opens it before its runner starts
([`runner-token.sh`](../../nix/modules/forge/runner-token.sh)). The old
`forgejo-runner-token` and `forgejo-members-runner-token` keys still in
node1's and node2's files are read by nothing.

`secrets/fleet.yaml` is nested:

```
boxes.<box>.owner       who owns the box
boxes.<box>.site        the house behind the box's siteId
boxes.<box>.region      the region behind its regionId
boxes.<box>.contents[]  what is kept on it ({ state })
sites.<siteId>          a house's name
regions.<regionId>      a region's name
cache.keyId, cache.keySecret, cache.signingKey   the cache writer, for publish
console-password        the passphrase for admin at a box's keyboard
console-password-hash   its hash, as copied into each box's file
```

Two programs read it, both with the laptop's key: `dd box list --private`
(the owner and place behind the ids in `fleet/boxes.json`) and `dd release
publish` (the cache writer's keys, [releases: the keys](releases.md#the-keys)).

## How a box reads its file

[`nix/roles/_sops.nix`](../../nix/roles/_sops.nix), imported by every bundle
that declares a secret (and by `core`, so by every box):

```nix
sops.defaultSopsFile = ../../secrets + "/${config.networking.hostName}.yaml";
sops.age.plugins = [ pkgs.age-plugin-tpm ];
sops.age.keyFile = "/var/lib/dd/secrets-key.txt";
sops.age.sshKeyPaths = [ ];
```

At activation sops-nix decrypts the file with the TPM key and writes each
declared secret to `/run/secrets/<name>` (tmpfs), and each template to
`/run/secrets/rendered/<name>`. The encrypted file is in the store, like
the rest of the system; the plaintext is never on disk.

The secrets key exists only on a box with `encrypted: true` in its
`box.json`: `report.nix` and `dd-secrets-key` are on only when
`dd.unlock.enable` is. A box without it cannot read its file.

The same key opens one more thing: the box's key to the fleet's network,
which the gateway hands over encrypted to the box's `secretsKey`
([`nix/modules/net/box.nix`](../../nix/modules/net/box.nix)).

## How a module reads a secret

A bundle declares the secret and passes its path to the module. The module
takes a path option, never a value:

```nix
# in the bundle
imports = [ ./_sops.nix ];
sops.secrets.tmdb-token = { owner = "dd-verify"; };
dd.verify.tmdbKeyFile = config.sops.secrets.tmdb-token.path;
```

The options used here:

```
sops.secrets.<name> = { };            /run/secrets/<name>, owned by root, 0400
sops.secrets.<name>.owner = "ente";   the user of the unit that reads it
sops.secrets.<alias>.key = "<name>";  the same value twice, for two owners
                                      (photos: ente-ott for ente, ente-ott-verify
                                      for dd-verify)
sops.secrets.<name>.neededForUsers    decrypted before users are made
                                      (console-password-hash)
sops.templates."<file>".content       a file built from several secrets, with
                                      ${config.sops.placeholder.<name>}
sops.templates."<file>".owner         its reader
config.sops.templates."<file>".path   where it lands
```

A template is how a program that wants an environment file gets one
(`garage.env`, `restic.env`, `cloudflare.env`). A unit reads the file with
`EnvironmentFile`, or is handed it with `LoadCredential` (cloudflared).

Nothing here sets `restartUnits`, so a unit sees a changed value the next
time it starts, not when the release that changed it lands. Restart it on
the box when it has to take the new value at once.

In VM tests there is no sops. A test passes the module a store file
(`pkgs.writeText "token" "test-token"`), or lets the box make its own, so
a test never needs a real secret.

## Where each box's secrets are going

Roles are gone (a box carries what its capability allows, and the
control plane switches each service on where it is placed), and with them
go the secrets a bundle brings in a box's file: a box that takes a service over
has no file with that service's secret in it. Each secret goes one of four
ways.

**Made on the box.** The box makes it the first time it needs it, keeps it
on its own disk, and nobody else has it: a box that takes the service over
makes its own.

```
pihole-api-password      dd-pihole-key (adblock/pihole.nix): Pi-hole and the gate here
nix-cache-signing-key    dd-cache-key (storage/cache.nix): what this box uploads, its
                         public half beside it (/var/lib/dd-cache-key/signing.pub),
                         which this box alone believes (its nix.conf there); the old
                         key in fleet/cache-keys.pub stays believed for what it signed
ente-key-*, ente-jwt-secret, ente-ott, garage-key-id/-secret
                         dd-ente-keys, with failover (photos/ente.nix)
vault-key-id/-secret, vault-oidc-secret
                         dd-vault-keys, with failover (vault/vaultwarden.nix)
cloudflared-credentials  dd-door's own tunnel, with failover (gate/public.nix)
```

**Its bucket moves into the box's own store**, whose keys `dd-store-setup`
makes on the box ([storage](storage.md)), and the shared pool's key goes
with it. Each needs its data copied over first, by `dd-store-migrate`,
copying and never deleting, and room for it in the store:

```
garage-backup-key-id/-secret   backups-<box>, into the store's own backups bucket
thanos-key-id/-secret          metrics: each box's blocks into its own store; the
                               observe box reads the copies the plan sends it
cache-key-id/-secret           nix-cache: the cache a box reads is its own store's
library-key-id/-secret         libraries: a bucket of the library's holder, as ente's
garage-rpc-secret              retires with the shared pool
```

**Sealed to the box by the box that holds it.** The holder seals the
value with age to each box able to take the service over (their
`secretsKey`, the recipients postgres-follow seals its archive key to),
beside the service's data in its own store, which the plan copies to
those boxes; the next holder opens its own copy with its TPM key as it
starts. The first holder's value comes from its file once, then the entry
goes from the file.

```
ente-demo-password       the demo gallery's account: in the ente bucket, sealed by
                         museum's holder
forgejo-runner-token,    made by the forge's holder (forgejo actions generate-runner-token)
forgejo-members-runner-token
                         every hour, sealed to each box built with a runner and
                         served beside the forge (git.<domain>/_dd/runner-keys/
                         <box>.<host|members>.age); a runner fetches and opens its own
                         before it starts, and needs it only to register
restic-password          made on the box, added to its repository with `restic key add`
                         under the old one before the box reads it (above), and sealed
                         to its owner's paper key (fleet/policy.json people.<owner>.paperKey)
                         beside the repository, so the owner can restore a box that died
wifi-psk                 the house's: sealed to each box of the house by the app's Wi-Fi
                         change and the installer (box.json house.sealedWifi), as a
                         joined box has it already
```

**The fleet's own, with an outside service.** A token another company
issued, one per fleet. They stay in the holder's file; a box that takes
the service over runs without it (`dd.hasSecret`) until the owner puts it
in that box's file:

```
cloudflare-workers-read  the Worker checks (Cloudflare)
tmdb-token               posters on Movies & TV (TMDB)
console-password-hash    the owner's passphrase at a box's keyboard, every box
```

Until a secret goes one of these ways, the bundle of modules that reads
it ([`nix/roles/bundles.json`](../../nix/roles/bundles.json) `secrets`: storage's pool keys, the runners'
tokens, the libraries' key) is built only on a box whose own file holds
it, and so the control plane places its services nowhere else
(`nix/lib/fleet-placement.nix`). That is the one thing still set by hand
about where a service may run, and it goes with the secret.

No longer read by anything, and to be taken out of `secrets/node1.yaml`:
`ente-smtp-password`, `alert-recipient` and `alert-smtp-password` (mail
goes through the mail Worker), `github-mirror-token` (no mirror), and
with failover on the ones made on the box above (`ente-key-encryption`,
`ente-key-hash`, `ente-jwt-secret`, `ente-ott`, `garage-key-id`,
`garage-key-secret`, `vault-key-id`, `vault-key-secret`,
`vault-oidc-secret`, `cloudflared-credentials`, `pihole-api-password`,
`nix-cache-signing-key`; node2's too for the last).

## Editing

`dd secret` ([`client/cli/src/main.rs`](../../client/cli/src/main.rs)):

```
dd secret init          make the editing key in this machine's credential store
                        (once), print its recipient
dd secret recipient     print the recipient of the key held here
dd secret run -- <sops arguments>
                        run sops with SOPS_AGE_KEY set to that key: sops from
                        PATH, else the repo's pinned one (nix run .#sops)
```

`nix develop` has sops, age and age-plugin-tpm on PATH. Run these from the
top of the checkout.

### Add or change a secret

1. Make the value where it belongs (a token from the service, a key from
   garage, a random string), without echoing it to the terminal.
2. Put it in with the value on stdin, as a JSON string, so it is not on a
   command line or in shell history:

   ```
   openssl rand -hex 32 | jq -R . | dd secret run -- set --value-stdin secrets/node1.yaml '["new-secret"]'
   ```

   Or open the file in sops's editor: `dd secret run -- edit secrets/node1.yaml`.
3. Declare it in the bundle (`sops.secrets.new-secret = { };`, with `owner`
   if a non-root unit reads it) and pass its path to the module.
4. Commit the encrypted file with the bundle. A secret its modules read
   whatever else goes in its `secrets` in `nix/roles/bundles.json`. The secret has to be in the
   file before the release that declares it reaches the box, or sops-nix
   fails the activation and the box rolls back.
5. If another box needs its own value, put a value of its own in that
   box's file. Never copy one box's value into another's file unless the
   secret is shared by design (`garage-rpc-secret`).

### Rotate a secret

1. Make the new value at its source first, where the source allows two at
   once (a second token, a second key), so the old one keeps working until
   the release lands.
2. Set the new value as above, and release.
3. Revoke the old value at its source once the box runs the release.

**A restic password is different.** The value opens an existing
repository, so a new one has to be added to the repository before the box
reads it. node2's backups failed for three days when its password changed
without this (2026-09-26 to 28). On the box, before the release:

1. `sudo restic-dd snapshots`: is there a repository already?
2. If there is, write the new value to a 0600 file in `/run` and
   `sudo restic-dd key add --new-password-file <file>`.
3. Set the same value in `secrets/<box>.yaml` and release.
4. When `restic-backups-dd` has run with the new value,
   `restic-dd key list`, `restic-dd key remove <old id>`, and shred the file.

### A new box's recipient

1. The box boots our system with `encrypted: true`; `dd-secrets-key` makes
   its TPM key and the report carries `secretsKey`.
2. `scripts/box-report <box>` copies the report into the repo
   ([scripts](scripts.md#box-report)).
3. Add the recipient to `.sops.yaml` as `&<box>-tpm`, and a creation rule
   for `secrets/<box>.yaml` with `<box>-tpm`, `david` and `david-dd`.
4. Create the file with the box's secrets (`wifi-psk` and
   `console-password-hash` at least), or, for a box whose TPM key changed,
   re-encrypt it: `dd secret run -- updatekeys secrets/<box>.yaml`.

A reinstall keeps the key: `scripts/install-box` carries
`/var/lib/dd/secrets-key.txt` across.

### A new editor

1. `dd secret init` on their machine; it prints the recipient.
2. Add it to `.sops.yaml` under `keys` and in each creation rule they should
   read.
3. Someone who can already decrypt runs
   `dd secret run -- updatekeys secrets/<file>.yaml` for each file.

## What CI checks

- **style** fails on any file under `secrets/` whose leaf values are not
  all `ENC[...]`, or that has no `sops:` block: a plaintext value would be
  published.
- **lint** runs gitleaks over the pull request's changes.
- A change under `secrets/` turns on CI's nix part (every box is built),
  not the VM tests.

Neither can tell a wrong value from a right one. A secret a box cannot use
shows up when the box takes the release: the unit fails, and the box rolls
back.

## Never

- **Never print a secret.** Not in a terminal, a log, a pull request or a
  page. Read names with `yq 'keys'` on the encrypted file; decrypt a single
  value only into the program that needs it.
- **Never commit plaintext.** No `sops -d > file` inside the checkout, no
  test value that is also a real one.
- **Never put a value on a command line** you would not paste in public:
  use `--value-stdin`, `--value-file` or the editor.
- **Never make a box a recipient of another box's file, or of
  `fleet.yaml`.** No box is trusted with more than its own secrets.
- **Never change a box's restic password without `key add` first** (above).
- **Never drop a recipient without `updatekeys`.** Removing a key from
  `.sops.yaml` changes nothing until the files are re-encrypted, and a
  value that key could read must be rotated: the old ciphertext is in the
  history for good.
