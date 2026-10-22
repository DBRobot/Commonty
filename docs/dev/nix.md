# Nix modules and tests

`nix/` is what a box is made of. The flake at the top of the tree builds
one NixOS system per box from three things: the box's folder in
`fleet/boxes/`, its entry in `fleet/boxes.json`, and the bundles and
modules in this folder. Which bundles a box is built with comes from its
capability ([`nix/lib/fleet-placement.nix`](../../nix/lib/fleet-placement.nix)),
never from a list. This page is how that folder is laid out, how a box
is put together from it, how it is tested, and the rules that came out of
releases that rolled back.

What runs inside the units (our programs, the pages) is
[`servers/`](servers.md). How a built system reaches a box is
[releases](releases.md). How CI builds and tests all of this is [CI](ci.md).

## The layout

```
nix/
  modules/<area>/        NixOS modules, one folder per area
    <program>.nix        options.dd.<thing>, the units, nginx, ports
    needs.nix            what each service in the area needs of a box (typed)
    *.sh, *.py           a unit's script, beside the module that runs it
  modules/mail.nix       dd-sendmail: a box's mail, TPM-signed, to the mail Worker
  modules/member-mail.nix  mail to members at <name>@<domain>
  roles/
    <bundle>.nix         one bundle: the modules it imports, its secrets, tile, backups
    bundles.json         what each bundle brings: services, secrets, with, why
    _sops.nix            the sops setup every bundle that reads a secret imports
    _member-mail.nix     member mail, for bundles that write to members
    README.md            the bundles in one list
  lib/
    needs-type.nix       the type every needs.nix is checked against
    needs.nix            every needs.nix, merged; .#needs is this as JSON
    capability.nix       what each box can run, from its report, box.json, policy
    fleet-placement.nix  what each box is built with (carried, built, carries),
                         and a joined box's (joined); the placement inputs
    vm-test.nix          wraps a test's .nix and .py into a NixOS VM test
    script.nix           ddScript: a unit's script from a file, with variables
  tests/
    <name>.nix, <name>.py  a VM test: the boxes, and the scenario
    box.nix              the smallest box a VM test builds on
    ci.nix, boxes.nix, placement.nix, nginx.nix, plaintext-label.nix,
    box-reports.nix      evaluation checks (no VM)
    control/, mail-worker/, dns-worker/, sso/, unlock/, box-reports/   helpers for one test
  app-release/           the app's downloads: Linux, Windows, Android, and their tests
  overlays/fhsenv.nix    nixpkgs' FHS builder made deterministic (games)
  patches/               patches to crates the flake builds
  laptop/                udev rule and units scripts/laptop-setup installs
```

## How a box is put together

[`flake.nix`](../../flake.nix) reads [`fleet/boxes.json`](../../fleet/boxes.json)
and makes `nixosConfigurations.<box>` for each entry with `mkBox`. There is
no Nix file per box. A box is data:

```
fleet/boxes.json            per box: hostKey, net, public, siteId, regionId,
                            garage, unlockKey
fleet/boxes/<box>/
  hardware-configuration.nix  what nixos-generate-config found (the box's report)
  hardware.json               disks, links, GPUs, firmware, TPM, secretsKey (the box's report)
  report.sig                  the box's TPM signature over the two above
  box.json                    the owner's choices: owner, house, encrypted,
                              extraPools, neverSleep, datasets (and optional
                              pools, gamesMemoryMiB, arcMaxBytes)
```

`mkBox` puts these modules together, in this order:

1. **The box's folder**, through
   [`nix/modules/box/host.nix`](../../nix/modules/box/host.nix). It imports
   `hardware-configuration.nix` and the disk layout
   ([`fleet/layout.nix`](../../fleet/layout.nix)) on `hardware.json`'s
   `bootDisk`, and turns the two JSON files into options: `dd.box.threads`,
   `dd.box.memoryBytes`, `dd.box.disks`, `dd.box.gpus`, `dd.box.vaapi` (the
   first GPU that encodes H.264), `dd.box.owner`, `dd.box.bulkPool`,
   `dd.zfs.datasets` (from `box.json`'s `datasets`, without their `note`),
   `dd.home` (the house Wi-Fi), `networking.hostId`, and graphics drivers
   when there is a GPU. It also declares the `wifi-psk` secret, so every
   box reads its own secrets file.
2. **What the box list says**: the host name, `dd.box.net`, `site`,
   `region`, the gate's peers (every other box's directory URL), the house
   peers (other boxes with the same `siteId`), the whole fleet for the
   Network tab, and `dd.agent.reach` (each other box's 4181 and 443, which
   the agent checks a switch did not cut off).
3. **Gateway extras**, on a box built with headscale: game ports for the
   network's rules, the owners of each box's address, and each box's
   unlock key and secrets recipient for headscale.
4. **`nix/roles/<bundle>.nix`** for every bundle it is built with
   (`fleet-placement.nix` `carried`), with the placed services it carries
   (`dd.placed.services`) and every service it is built with (`dd.built`).
5. **Derived wiring**: garage's zone, capacity and peers on a box built
   with garage (`garageOf`); the nix cache and S3 address, local on such a
   box, else the first one's over the fleet's network (`cacheOf`); the
   backup endpoint for a box without garage (`backupEndpoint`).
6. **What every box gets**: lanzaboote and
   [`secure-boot.nix`](../../nix/modules/box/secure-boot.nix),
   [`disk-unlock.nix`](../../nix/modules/box/disk-unlock.nix),
   [`control.nix`](../../nix/modules/box/control.nix) (enabled),
   [`ports.nix`](../../nix/modules/box/ports.nix),
   [`report.nix`](../../nix/modules/box/report.nix), disko, and sops-nix
   when any of its bundles imports `_sops.nix` (`core` does, so today
   every box).
7. **Thanos**, on a box built with the fleet view: its own sidecar only.

A box that joined on its own key is built the same way
(`lib.joinedBoxModules`), from its own report and box.json
(`fleet-placement.nix` `joined`), with only the bundles of what a
member's box may run (placement-inputs `joined`).

Every module gets three extra arguments (`specialArgs`): `self` (the flake,
for `self.packages`), `vmTests` and `boxNames`, and `ddScript`.

`dd.domain` and `dd.repo` come from [`fleet/policy.json`](../../fleet/policy.json)
in `core.nix`. The member list (`fleet/members.json`) and the release key's
public half (`fleet/release.pub`) are built into every box there too.

## Bundles

A bundle is modules that move together, with what they read from sops,
their tile on the home page (`dd.home.services`) and the paths to back up
(`dd.backup.paths`). The list is in
[`nix/roles/README.md`](../../nix/roles/README.md); what each brings, the
secrets it reads and the bundles it needs beside it are in
[`nix/roles/bundles.json`](../../nix/roles/bundles.json)
([servers: bundles](servers.md#bundles)). The folder is still called
`roles/`; nothing hands a box one.

```
core      every box: domain, box facts, the gate as a directory replica, metrics,
          disks, datasets, usage counts, postgres isolation and dumps, backup,
          the release agent, thanos sidecar, locate, the fleet's network,
          owner access; admin user, console password, firewall, nix gc and limits
gateway   acme, the public front door, the mail Worker check, headscale; the
          gate as the full verifier
storage   garage and its secrets; restic's password and key; thanos and cache keys
photos    ente, the storage ledger
media     user accounts, webdav-media (Movies & TV, Files)
library   the libraries bucket behind the gate, the transcoder
llm       llama-cpp, the Chat page, search
game      the games manager
forge     forgejo
runner    the forge's runner, the members' runner, the cache's signing key
observe   the Metrics page, alerting, mail
vault     vaultwarden, the breach list, member mail
adblock   Pi-hole for the house
```

Files whose names start with `_` are not bundles. They are pieces bundles
import: `_sops.nix` and `_member-mail.nix`. `checks.boxes` fails if a box
is built with a bundle with no file, lacks `core`, or names a role in
`fleet/boxes.json`.

[`core.nix`](../../nix/roles/core.nix) holds what every box is before any
other bundle:

- the `admin` user with the owner's ssh keys from the policy, and a console
  password whose hash comes from sops (`neededForUsers`, with
  `users.mutableUsers = false`, or the hash never reaches `/etc/shadow`);
- the firewall on `commonty0`: 443 to everyone on the fleet's network;
  3900 and 3901 (garage) from the other boxes' addresses only; 4181 from
  the same house's boxes only;
- NetworkManager with `no-auto-default = "*"`, and docker's interfaces left
  alone;
- `nix.gc` weekly, keeping 14 days; nix-daemon at `MemoryHigh` 60% and
  `MemoryMax` 75% of RAM, and with the build orders and the runner in
  `work.slice` under all of RAM but a fifth (at least 3G kept back);
  `max-jobs` one per 6G, at most threads/4;
  `min-free` 10G, `max-free` 30G;
- journald kept 14 days and 1G at most;
- `dd.backup.paths` for the gate's state and Prometheus.

## Modules

One folder per area. A module declares `options.dd.<thing>`, and is enabled
by a bundle or by `core`.

```
adblock/   pihole.nix: Pi-hole and unbound for the house
box/       what makes a box: host.nix (the box's folder), box.nix (dd.box.*),
           domain.nix (dd.domain, dd.repo), agent.nix (release agent, build
           orders), control.nix, disk-unlock.nix (+ its scripts),
           secure-boot.nix, report.nix (+ report/detect.sh), ports.nix,
           zfs-datasets.nix, home-network.nix (+ wifi scripts), locate.nix
forge/     forgejo.nix, forgejo-runner.nix, members-runner.nix,
           live.nix (the Git pages' events); ci-cancel.py,
           ci-resume.py, forgejo-setup.sh, runner-address.sh
games/     games.nix (the manager), game-guest.nix (the VM a world runs in),
           game-run.py, games-access.sh, games-measure.sh
gate/      verify.nix (the gate's unit, nginx's sign-in check, dd.verify.*,
           dd.home.services), acme.nix (and acme-ask.sh, acme-exec.sh: a
           challenge asked of the DNS Worker), public.nix (the tunnel, dd.public.*),
           headers.nix, strip-cookie.nix, mail-worker.nix, dns.py
library/   libraries.nix (the bucket behind the gate), transcode.nix
llm/       llama-cpp.nix, llm.nix (the Chat page), search.nix (SearXNG)
media/     webdav-media.nix
net/       box.nix (a box on the fleet's network), headscale.nix, owner.nix
observe/   metrics.nix, disks.nix, thanos.nix, metrics-ui.nix, alerting.nix
photos/    ente.nix, ente-health.sh
storage/   garage.nix, backup.nix, cache.nix, cache-gc.sh, postgres-backup.nix,
           postgres-isolation.nix, usage.nix, ledger.nix, view.nix
vault/     vaultwarden.nix, pwned.nix
```

Each service's own page says more: [the gate](gate.md), [the forge](forge.md),
[games](games.md), [libraries](libraries.md), [packaged
programs](packaged.md), [the control plane](control-plane.md),
[releases](releases.md).

### The options touched most

```
dd.domain, dd.repo         the base name and the fleet's repo (policy.json)
dd.box.net/site/region     where the box is (boxes.json)
dd.box.threads/memoryBytes/gpus/vaapi/disks   from the box's report
dd.box.owner, dd.box.bulkPool                 from box.json
dd.box.plaintext           services here that read people's plaintext (a label)
dd.box.s3                  the S3 address for this box
dd.ports.<service>         the fixed ports a unit listens on, below 32768
dd.zfs.datasets            "pool/dataset" = { property = value; }
dd.zfs.atMountpoint        "/path" = { property = value; } on whatever is mounted there
dd.backup.paths/exclude    what restic keeps of this box
dd.home.services           a tile: name, url, icon, color, blurb, rank, menuOnly, demo...
dd.verify.hosts            subdomains the gate stands in front of (lists add up)
dd.public.hosts            subdomains reachable from outside the fleet's network
dd.usage.<service>         a member-usage query a service runs on its own database
dd.<service>.enable        the service itself
```

`nix eval` shows what a box ends up with, without building anything:

```
nix eval .#nixosConfigurations.node1.config.dd.ports --json
nix eval .#nixosConfigurations.node2.config.dd.backup.paths --json
nix eval .#nixosConfigurations.node1.config.systemd.services.dd-agent.restartIfChanged
```

### A unit's script

A script longer than a few lines lives in a file beside its module, not in
a Nix string. `ddScript ./file.sh { NAME = value; }`
([`nix/lib/script.nix`](../../nix/lib/script.nix)) prepends one
`export NAME='value'` line per variable, shell-escaped, to the file's text.
The file reads as shell, and CI's lint runs shellcheck on it (every changed
`.sh` file). A Python script is a `.py` beside the module too, run as
`${pkgs.python3}/bin/python3 ${./ci-cancel.py}`, and linted by ruff.

## Needs

Each area has a `needs.nix`: per service, what it is, what it uses, what
it requires and what makes it faster, who reaches it and what it reaches,
what it depends on, and what of people's data it can read. The type is
[`nix/lib/needs-type.nix`](../../nix/lib/needs-type.nix); a field it does
not know fails evaluation.

```
<service> = {
  what         one line
  everyBox     runs on every box, whatever else it is built with (placement does not choose it)
  perHouse     at most this many in one house
  instances    at most this many at once (null: any)
  servesCode   the box hands people code that handles their secrets
  base = {
    uses       memory, storage ("0", "28M", "1.5G"; binary units), estimated
    requires   it does not run without these
    faster     it runs without these, slower
               both take: arch [x86_64 aarch64], gpu { api vulkan|vaapi,
               encode, memory }, memory { fixed, perKContext, locked },
               disk { kind ssd|any, size.min }, place any|house, threads,
               windowsLicense, kvm, nestedKvm, uefi, wiredInitrd, tpm
    network    outbound: none | internet | [ hosts ]
               inbound:  none       nobody; it only calls out
                         localhost  only this box
                         lan        the house network (and its owner's devices)
                         network    the Commonty network
                         public     anyone
    depends    <service>.local = true|false (on this box, or anywhere)
    privacy    none | metadata | plaintext
  };
  variants.<name> = { ... }   the same parts, added to the base; only the base must say everything
}
```

`requires.uefi`, `requires.wiredInitrd` and `requires.tpm` are what the
disk unlock needs: a signed UEFI boot the TPM measures, a cable the initrd
has a driver for, and a TPM 2.0 with P-256 keys. `uses` is measured where
the service runs (systemd's memory peak, `du` of its state), with the date
and box in a comment; `estimated = true` until it has run anywhere.

[`nix/lib/needs.nix`](../../nix/lib/needs.nix) lists every file and merges
them through the type. A new `needs.nix` goes in its list. `.#needs` is the
merged set as JSON:

```
nix build .#needs && jq 'keys' result
```

What a box is built with comes from the needs:
[`capability.nix`](../../nix/lib/capability.nix) holds each service's
`requires` to the box's signed `hardware.json`, its owner's `box.json`
and the policy (`trust`, `ownHardware`, a metered line), and
[`fleet-placement.nix`](../../nix/lib/fleet-placement.nix) builds each box
with the bundles of what it can run whose secrets its file holds
([what each service needs from a box](hardware.md)).

## Tests

All of them are `checks.x86_64-linux.<name>` in the flake.

### Evaluation checks

Seconds each, no VM:

```
boxes            no box names a role; every bundle it is built with is a file,
                 every box has core and builds; peers, garage addresses, thanos
                 and llm wiring say what it is built with; two services on one
                 port fail (ports.nix)
placement        every box joins, carries only what it can run, runs only what
                 its bundles bring; a name follows one service
box-reports      fleet/boxes/<box>/ is what the box signed (box-reports/check.py)
nginx            every box's nginx config through `nginx -t` (nginx-t.py)
plaintext-label  a module that reads plaintext registers in dd.box.plaintext
ci               the CI matrix names exactly the flake's vmTests; ci_ok waits
                 on every job and runs with if: always()
control          the control plane offline (control/run.py)
mail-worker      the mail Worker in a browser against a fake Cloudflare
safe-rd          where the sign-in pages send you afterwards stays on the site (safeRd)
dns-worker       the DNS Worker against a fake Cloudflare, and the box's helper
sso              sign-in once for every box: the gate and the id Worker in a browser
```

### VM tests

A VM test boots one or more boxes from the same modules the real boxes
import and drives them. It is two files:

- **`nix/tests/<name>.nix`**: a set for `runNixOSTest`: `name`, `nodes.<box>`
  (usually importing [`./box.nix`](../../nix/tests/box.nix) and the module
  under test), `defaults` for memory and cores, and optionally `scriptEnv`.
  Secrets are test values in the store (`pkgs.writeText`); no test reads
  sops. It gets `pkgs`, `self` and `lib`.
- **`nix/tests/<name>.py`**: the scenario, as Python for the NixOS test
  driver (`start_all()`, `box.wait_for_unit(...)`, `box.succeed(...)`,
  `with subtest(...)`).

[`nix/lib/vm-test.nix`](../../nix/lib/vm-test.nix) joins them. When the
`.nix` has a `scriptEnv`, the script starts with one line,
`nix = <scriptEnv as JSON>`, so the Python reads `nix["dd"]` instead of
having store paths pasted into it. `scriptEnv` can be a function of
`{ nodes }` when it needs a node's built system (`release.nix` does). Every
node also gets `ddScript`, `vmTests` and `boxNames`, as a real box does.

`box.nix` is the gate as a directory replica and metrics, on
`test.invalid`, with the firewall off and a 2-second directory sync. A test
that needs the full gate sets `dd.verify.role = lib.mkForce "full"`.

Run one:

```
nix build --no-link .#checks.x86_64-linux.adblock
nix build --no-link -L .#checks.x86_64-linux.adblock    # with the log
nix build .#checks.x86_64-linux.adblock.driverInteractive && ./result/bin/nixos-test-driver
```

On Ubuntu, nix's build users need `/dev/kvm` or the VMs run under
emulation; `scripts/laptop-setup` installs the udev rule. A VM test takes
memory: run one at a time on a laptop, or on a box.

### Adding a VM test

1. Write `nix/tests/<name>.nix` and `nix/tests/<name>.py`. Start from a
   test close to it (`adblock` for one box and a service, `backup` or
   `thanos` for two boxes). Drive the failure cases as well as the working
   one.
2. Add `"<name>"` to `vmTests` in [`flake.nix`](../../flake.nix).
3. Add `<name>` to `jobs.vm_tests.strategy.matrix.test` in
   [`.forgejo/workflows/entry_point.yml`](../../.forgejo/workflows/entry_point.yml).
   `checks.ci` ([`nix/tests/ci.nix`](../../nix/tests/ci.nix)) compares the
   two lists, sorted, and fails while they differ.
4. Run it locally once, and say how long it took in the pull request.

An evaluation check is added to `checks` in `flake.nix` by hand
(`import ./nix/tests/<name>.nix args`) and run by CI's nix part; it needs
no matrix entry.

## The app's downloads, overlays and patches

- **[`nix/app-release/`](../../nix/app-release)** builds the app for a
  version: `linux.nix` (the .deb and AppImage, inside a stock Ubuntu 22.04),
  `windows.nix` (the installer and `dd.exe`, cross-built, with
  `nsis-hooks.nsh`), `android.nix` (the apk; gradle offline against
  `android-deps.json`), and `test-linux.nix`, `test-windows.nix`. The flake
  exposes them as `legacyPackages.x86_64-linux.appRelease "<version>"`.
  [Releasing the app](app-release.md) is how they are used; read it before
  changing anything here.
- **[`nix/overlays/fhsenv.nix`](../../nix/overlays/fhsenv.nix)** patches
  nixpkgs' FHS rootfs builder to break ties by path instead of hash-map
  order, so two builds of `steam-run` come out the same. The boxes vouch for
  a release by rebuilding it, so a nondeterministic build fails attestation.
  The games module applies it.
- **[`nix/patches/`](../../nix/patches)** holds
  `i18n-embed-fl-args-in-order.patch`, applied to the `i18n-embed-fl` crate
  in `crateOverrides`: `fl!` wrote arguments in hash-map order, and `dd`
  differed on every build. Patches to upstream programs we package are not
  here; they live with the service in `servers/<service>/`.
- **[`nix/laptop/`](../../nix/laptop)** is installed onto an Ubuntu laptop
  by [`scripts/laptop-setup`](scripts.md#laptop-setup), not built into any box.

## State on a box

The modules keep their state in fixed places. The ones a developer meets
most:

```
/var/lib/dd/secrets-key.txt   the TPM handle that decrypts secrets/<box>.yaml
/var/lib/dd/report/           the box's signed report on itself, made each boot
/var/lib/dd-agent/            the release agent: counter, applied, pending, refused
/var/lib/dd-verify/           the gate: directory, sessions, sealed library keys (backed up)
/var/lib/dd-control/          the control plane, and the box's vote key
/var/lib/dd-facts/            *.prom files the node exporter reads
/var/lib/dd-usage/<service>/  each service's member usage counts
/var/lib/commonty-net/        the box's node on the fleet's network
/var/lib/prometheus2/         metrics, backed up but for the WAL
/run/secrets/<name>           a decrypted secret (sops-nix); rendered/ for templates
/etc/dd/plaintext             dd.box.plaintext, one line each
```

Datasets on the pools come from `box.json` and from modules
(`dd.zfs.datasets`), and are created by the `zfs-datasets` oneshot.

## Rules that came from rollbacks

A box keeps a release only if every unit comes up during the switch and
after it ([what the agent does](releases.md#what-the-agent-does)). Each
rule below is a release that rolled back.

- **A timer unit never fails a switch.** Its first run is a few minutes
  after its timer starts (`OnActiveSec`, not `OnBootSec`, which fires at
  once on the switch that adds the timer). A switch does not restart it
  (`restartIfChanged = false`). Something it needs not answering yet is
  "next time": exit 0. Releases 212, 213 and 224 rolled back on
  `dd-build`, `dd-ci-resume` and `dd-storage-view`
  ([CI](ci.md#when-a-release-does-not-land)).
  [`storage/view.nix`](../../nix/modules/storage/view.nix) is a unit
  written this way.
- **`zfs-datasets` sets only what differs.** Any change to
  `dd.zfs.datasets` changes the oneshot's script, so a switch runs it
  again. It used to `zfs set` every property, and setting a mountpoint to
  its own value unmounts the dataset: Forgejo's was busy, the unit failed,
  Forgejo stayed down and the release rolled back (release 160, fixed in
  161). Before shipping a change to datasets, dry-run the generated script
  on the boxes with `zfs set` and `zfs create` replaced by `echo`:
  `nix eval --raw .#nixosConfigurations.<box>.config.systemd.services.zfs-datasets.script`.
- **Ports are declared.** Two services on one port fail the build, not the
  switch (release 172: dd-control took the transcoder's 4190). A new
  listener goes in `dd.ports`; the hourly `dd-ports-check` names one that
  is listening and was never declared.
- **nginx configs are parsed before release.** Building a config does not
  check its syntax; release 115 shipped a location regex nginx could not
  read. `checks.nginx` runs `nginx -t` on every box's config.
- **The agent's own units are never restarted by a switch.** `dd-agent`
  and `dd-build` have `restartIfChanged = false` and `stopIfChanged = false`.

## Building

- **Build from a commit.** A `nix build` on a dirty worktree copies the
  whole tree into the store, ignored files included (a `target/`
  directory, build outputs), as a new `-source` path each time. Two
  sessions filled the laptop's disk this way. Commit first, or build
  `git+file://$PWD?ref=<branch>`, as the scripts do. `--no-link` leaves no
  `result` link holding a closure alive.
- **Something committed that should not be.**
  `git ls-files -i -c --exclude-standard` lists tracked files `.gitignore`
  says to ignore. The Rust source filters take everything under `app/`,
  so a build output committed there goes into every build.
- **A box's system**:
  `nix build --no-link .#nixosConfigurations.node1.config.system.build.toplevel`.
  This is large; on a laptop, build one at a time.
- **`nix flake check --no-build`** evaluates every check without building.

## Changing it

### A module option

1. Add it to the module's `options.dd.<thing>` with a type and a
   `description` that says where its value comes from.
2. Set it in the bundle, or in `host.nix` when it comes from the box's folder.
3. `nix eval` it on each box it touches, then run the area's VM test.
4. If a page documents the option, change it in the same pull request.

### A bundle

1. `nix/roles/<bundle>.nix`: imports (`./_sops.nix` first if it has
   secrets), `enable = true`, its secrets, its tile, its `dd.backup.paths`.
2. The bundle in [`nix/roles/bundles.json`](../../nix/roles/bundles.json)
   with its `services`, `secrets`, `with` and `why`, and in
   [`nix/roles/README.md`](../../nix/roles/README.md).
3. Every box able to run one of its services, whose file holds its
   secrets, is built with it; `checks.placement` and `checks.boxes` say
   which.
4. Its secrets in the files of the boxes that should have it, before the
   release ([secrets](secrets.md)).

### A unit on a timer

1. `OnActiveSec` for the first run, `OnUnitActiveSec` or
   `OnUnitInactiveSec` after it.
2. `restartIfChanged = false` on the service.
3. Every "not ready yet" path exits 0 with a line in the journal.
4. Run the module's VM test, and switch a box to it before release where
   the unit talks to something the same switch restarts.

### A box's datasets

1. Edit `datasets` in `fleet/boxes/<box>/box.json`, or `dd.zfs.datasets`
   in a module.
2. Read the generated script (`nix eval --raw ...zfs-datasets.script`),
   and run it on the box with `zfs set` and `zfs create` as `echo`.
3. Check that nothing it would set is a mountpoint of a dataset in use.

## Pitfalls

- The flake reads only tracked files. A new file that is not `git add`ed
  is missing from the build ("path does not exist"), even on a dirty tree.
- `nixfmt --check` runs on every changed `.nix` file in CI (style).
  Run `nix shell .#ci-tools -c nixfmt <file>`.
- `checks.boxes` builds every box's `drvPath`; a mistake in any bundle shows
  there first.
- A module that needs another's option checks it exists
  (`lib.optionalAttrs (options.dd ? games) ...`), as `host.nix` does, so a
  box without that bundle still evaluates.
- VM tests take secrets as store files. Something that works in a test
  can still fail on a box because the secret's `owner` is wrong; set
  `owner` on every secret a non-root unit reads.
