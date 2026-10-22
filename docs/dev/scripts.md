# Box scripts

`scripts/` is the shell a person runs from a checkout on their own
machine: copying a box's report into the repo, putting a box back
together, setting up the laptop, attacking the public face, and keeping
the app's copies of the pages in step. Nothing here runs on a box by
itself, and nothing here is part of a release. A box takes releases through
its agent ([releases](releases.md)); these are for the steps around that.

## The layout

```
scripts/
  box-report      copy a box's signed report into fleet/boxes/<box>/
  sync-app-web    copy the pages the app carries into app/web/_dd/
  laptop-setup    /dev/kvm for nix's builders, and the laptop on the fleet's network
  stress-public   what a stranger on the internet can and cannot do to the front door
  deploy          switch one box to one git ref by hand, with a rollback armed
  install-box     reinstall a box from scratch, keeping its identity
  restore-box     after install-box: put back what the boot disk held
```

All are bash, run from anywhere inside the checkout (each finds the
repository from its own path). They are not shellchecked by CI: lint runs
shellcheck on `.sh` files, and these have no suffix. Run
`nix shell .#ci-tools -c shellcheck scripts/<name>` after changing one.

The ones that reach a box find it by its address on the fleet's network,
`net` in [`fleet/boxes.json`](../../fleet/boxes.json), and log in as
`admin` with your ssh key (the box owner's keys in
[`fleet/policy.json`](../../fleet/policy.json)). When the fleet's network
is what is broken, `deploy` and `restore-box` take `DD_ADDR`
(`DD_ADDR=admin@192.168.1.20`), and `install-box` always takes `DD_LAN`.

## box-report

```
scripts/box-report <box> [address]
```

Copies `hardware-configuration.nix`, `hardware.json` and `report.sig` from
`/var/lib/dd/report/` on the box into `fleet/boxes/<box>/`, then shows
`git diff --stat` for the folder. The box makes the report at every boot
and signs it with its TPM
([`nix/modules/box/report.nix`](../../nix/modules/box/report.nix));
`checks.box-reports` refuses a folder the box did not sign. So this script
is the only way those three files change. Nobody edits them by hand.

- **When:** after a box's hardware changes (a disk, memory, a GPU), after
  a reinstall, or when the report's format changes in a release.
- **Needs:** ssh to `admin@<net>` (or the address given).
- **Safety:** reads only. Each file is written to `.new` first and moved
  into place once all three have arrived. Commit the result in its own
  pull request; `checks.placement` then says what the box can run on it,
  and the release builds it with that.

## sync-app-web

```
scripts/sync-app-web
```

Copies every file [`app/carried`](../../app/carried) lists (`<source>
<path under app/web/_dd>`, one a line, `#` for comments) into
`app/web/_dd/`, then builds `.#web` and copies the wasm bundle and players
into `app/web/_dd/web/` (that folder is ignored by git; the app's own build
copies the same).

- **When:** after changing any page, script or stylesheet the app carries
  ([servers: the app's copies](servers.md#the-apps-copies)).
  `checks.stylesheet` fails while a copy differs from its source.
- **Needs:** nix, for the web bundle. Without nix it copies the pages and
  skips the bundle.
- **Safety:** writes only under `app/web/_dd/`. It does not delete a copy
  whose line was taken out of `app/carried`; `checks.stylesheet` fails on a
  file the list does not name, so remove that copy by hand.

## laptop-setup

```
scripts/laptop-setup
```

What an Ubuntu laptop with nix needs that no flake can set:

1. **`/dev/kvm` for nix's build users.** Installs
   [`nix/laptop/60-kvm.rules`](../../nix/laptop/60-kvm.rules) (mode 0666)
   and reloads udev. Without it, VM tests run under emulation.
2. **The fleet's network, as the owner's device.** Builds `.#netbridge`,
   pins it as the profile `/nix/var/nix/profiles/commonty-netbridge`, and
   installs and starts two units from [`nix/laptop/`](../../nix/laptop):
   `commonty-net.service` (a second tailscaled, its socket at
   `/run/commonty-net/tailscaled.sock`) and `commonty-net-bridge.service`
   (the bridge to headscale on `127.0.0.1:41643`). If the node is not up
   yet, it asks the gate for a join key with `dd token`
   (`POST https://home.commonty.org/_dd/network/join`), joins, and deletes
   the key file.
3. **Netfilter off on both tailscaleds.** Each installs a rule dropping
   100.64.0.0/10 traffic that did not arrive on its own interface, which
   drops the other's. ufw, if on, still applies.

- **When:** once per laptop, and again after the units in `nix/laptop/`
  change.
- **Needs:** sudo; `dd` signed in, for the join key; jq and curl.
- **Safety:** changes system files (`/etc/udev/rules.d`,
  `/etc/systemd/system`). The domain is written into the script and the
  bridge unit, not read from the policy.

## stress-public

```
scripts/stress-public [domain] [address]
```

Attacks the public front door from wherever it runs and prints PASS or
FAIL per check; the exit code is the number of failures. Run it from
outside the fleet's network (a phone's hotspot, a VPS) after every change
to what the public can reach. Inside the network, the wildcard name
resolves to the gateway's address on that network and the results mean
nothing. The address defaults to what `home.<domain>` resolves to; every
request is sent there by name (`curl --resolve`).

What it checks:

```
1  only 80 and 443 are open (needs nmap; skipped without it)
2  the certificate verifies; TLS 1.1 is refused; http redirects to https
3  home and accounts answer; photos, files, llm, git, grafana, games, s3,
   api, albums, cast and locker are shut to an outsider (000 or 403)
4  sign-in and sign-up pages load; /_dd/home without a session redirects;
   directory entries are public (a note, not a check)
5  a burst of 25 sign-in starts gets at least 5 refusals (429)
6  sign-up hands out a ceremony; a fake grant is refused
7  a 2 MB body is refused (413); a slow body is cut off within 30 s
8  the demo signs in, and cannot reach games from outside
```

- **Needs:** curl, dig, openssl; nmap for the port scan; `timeout`.
- **Safety:** it sends real requests to the live site, including about 25
  sign-in starts and one sign-up start (as `stranger-probe`), and a slow
  connection held for up to 45 s. It writes nothing on a box beyond what
  those requests do. Add a check here when a new public host, or a new
  public route, goes in.

## deploy

```
scripts/deploy <box> [branch]        branch defaults to main
```

Switches one box to the system built from a local branch, by hand,
without a release. Boxes normally move only to a signed release
([releases](releases.md)); this is for a box the release path cannot reach,
or a fix that has to land before a release can.

1. Refuses unless the local branch exists and equals `forge/<branch>`
   (push it first).
2. Builds `nixosConfigurations.<box>` from git
   (`git+file://<repo>?ref=<branch>`), never from the working tree.
3. Asks the box which store paths it lacks, accepts only paths from the
   built closure, and streams them over ssh (`nix-store --export | zstd`).
   The box needs no internet.
4. Arms a transient `deploy-rollback-guard` timer on the box: in 15 minutes
   it runs `nixos-rebuild --rollback switch`.
5. Sets the system profile and runs `switch-to-configuration switch` as a
   transient unit, detached from the ssh session, and prints any
   error lines from its journal.
6. Fails, leaving the rollback armed, unless `/run/current-system` is now
   the built path. Otherwise it disarms the guard.

- **Needs:** nix, jq, zstd and ssh here; `admin` with sudo on the box.
- **Safety:** the release agent moves the box back to the signed release
  on its next run (every 5 minutes): a hand switch does not stay unless
  `dd-agent.timer` is stopped on that box, and a stopped agent takes no
  releases until it is started again. A switch that fails part-way can
  leave services stopped even after the rollback; check
  `systemctl --failed` on the box.

## install-box

```
DD_LAN=<address> scripts/install-box <box> [branch]
```

Reinstalls a box onto the fleet's disk layout and keeps its identity. It
wipes the boot disk. Use it when a box's root has to be rebuilt (a new
layout, a broken system), never on a box that is fine.

1. Refuses unless the branch equals `forge/<branch>`. Builds the box's
   system and its disko script from git.
2. Takes the box's identity over ssh into a local tree: the ssh host key,
   `/var/lib/tailscale` and its certificates, garage's node key, the uid map
   (`/var/lib/nixos`), the gate's state (`/var/lib/dd-verify`), ACME
   certificates, the secrets key (`/var/lib/dd/secrets-key.txt`) and the
   control-plane vote key. Checks the host key is `hostKey` in
   `fleet/boxes.json`, and that no directory in the tree is group- or
   world-writable, before anything is wiped.
3. Kexecs the box into the NixOS installer with nixos-anywhere and finds
   the installer by the box's MAC on the shared network (it can come up
   on another address). Turns off USB autosuspend there.
4. Formats, installs with the identity tree as extra files, and reboots.
5. Waits for ssh, then checks: the same host key, the built system,
   root on ZFS, `/etc`, `/etc/ssh`, `/var` and `/var/lib` at 755, the host
   key root-owned 600, healthy pools, the fleet network address equal to
   `net` in `fleet/boxes.json` (or prints a new one to put there), and no
   failed units.

- **Needs:** `DD_LAN`, the box's address on a network this machine shares
  (the MAC lookup needs it); the box in `fleet/boxes.json` with its
  `hostKey`; nix with network access (it runs
  `github:nix-community/nixos-anywhere`).
- **Safety:** destroys the boot disk. Other pools are left alone; what
  lived on the boot disk comes back with `restore-box`. An aborted run
  during kexec kills the box's pending installer download, so it does not
  kexec on its own later. The secrets key is a handle into this box's TPM:
  without it the reinstalled box cannot read `secrets/<box>.yaml`.

## restore-box

```
scripts/restore-box <box>
```

Run after `install-box`. On the box, as root:

1. Waits for garage to see at least two nodes, then repairs its block and
   table indexes (the box's blocks are still on its data pool).
2. Stops the services whose state comes back, restores from the box's
   restic backup (`restic-dd --no-lock`, read-only, so it works with a box
   of the cluster down) the paths that lived on the boot disk:
   `/var/lib/grafana`, `/var/lib/prometheus2`, `/var/lib/garage/meta/snapshots`.
3. Loads the hourly postgres dumps from `/vault/backups/postgres/` for
   `ente` and `forgejo`, but only into a database whose marker table
   (`key_attributes`, `repository`) is empty; one with rows is left alone.
4. Starts the services again and lists failed units.

- **Needs:** ssh to the box (or `DD_ADDR`); the box's backup in the garage
  cluster; the other storage box up.
- **Safety:** a database is dropped and recreated only when its marker
  table is empty. A restic restore writes over the paths it restores, so
  run it before the box has gathered new state there.

## Changing them

1. Keep each script's header: usage, then what it does in numbered steps.
2. Build from git, never the working tree, in any script that builds.
3. Check the box is the box (its host key) before anything destructive.
4. Treat what a box answers as data: never split it into words on a
   command line here (`deploy` checks each path it is asked for).
5. Run shellcheck on it by hand, and the script once against a real box
   or a VM, before the pull request.
6. Change this page in the same pull request.
