# Home-Server

NixOS configuration for a self-hosted family server. Everything here is
declarative: the machines are rebuilt from this repository, and secrets are
committed encrypted rather than kept out of tree.

## Layout

    flake.nix          entry point; `nixosConfigurations.<box>` from fleet/boxes.json
    fleet/             what the fleet is: the boxes, the members
      boxes/<box>/     one box as data: its own hardware report, its owner's choices
    nix/               what a box is
      modules/         NixOS modules, grouped by concern (box, gate, forge, games, ...)
      roles/           bundles of modules; a box is built with what its capability allows
      tests/           VM tests: boxes booted and driven
    servers/           what runs on boxes: our programs (the gate, the release agent,
                       the games manager, the control plane), the pages, patches
    client/            Rust a person runs: the dd cli, its libraries, the browser wasm
    app/               the app: the client crates behind a window, copies of the pages
    data/              the games catalogue and covers
    scripts/           a box by hand (deploy, install-box, restore-box, box-report),
                       the laptop's setup, the public face's stress test, the app's pages
    secrets/           sops-encrypted values (safe to publish)

## Deploying

Boxes pull signed releases: what merges to `main` on the forge is published
with `dd release publish`, and each box's agent switches to it
([docs/dev/releases.md](docs/dev/releases.md)). By hand, one box from one
ref of the forge's repository: `scripts/deploy node1 [ref]`.

## TODO

- **No box talks to another box.** A box is reached by people and by
  services through its front door, the way any client reaches it, and never
  through a channel that exists because the other end is also a box. Today
  one remains, on the fleet's own network, open in its rules to boxes alone
  and fenced again by each box's firewall: inside one house, the Wi-Fi relay
  (4181), which goes with the agreed log. The stores' own port (3443) is
  each store's door, sealed keys and all.
- **Contributed boxes grow the pool.** Each box has its own store now, and
  copies are placed across them by room. What it still lacks: a bucket is
  kept whole on each box that keeps it, so the pool is as big as one box's
  store, not their sum. Erasure coding is the eventual shape and nothing off the shelf
  fits (Garage has none by design, MinIO is archived, SeaweedFS shares a
  signing key across volume servers, Tahoe-LAFS fits the trust model but not
  S3). It stops mattering until there are five or six houses; below that,
  replication is optimal anyway.
- **An off-site copy.** Every copy of every member's data is in one building.
- **One vote per house in the control plane.** It starts with three voters:
  node1, node2 and the Cloudflare witness (docs/dev/control-plane.md). That
  survives any one of them going down, but it is not Byzantine-safe (that
  takes four), and node1 and node2 share a house, so the house votes twice.
  As boxes go off site: each new house gets one validator and node2 stops
  voting; outside witnesses fill the count up to four in the meantime (a
  small VPS, or a member's always-on device in another place); and once
  there are four houses, the witnesses retire, the Cloudflare one last. A
  house that joins changes the validator list in a signed release, never on
  its own say.
- **Nothing readable by Cloudflare.** The tunnel ends TLS at Cloudflare, so
  today it sees and could change every page and request that comes in
  through the public front door. Accepted for now. The way out: TLS that
  ends on the box (a pass-through relay, or members over the fleet's network), with
  Cloudflare left carrying only what is end-to-end encrypted. Passwords is
  already there: the fleet's network only, TLS on the box, and Cloudflare sees none of
  it; its page stays on the box, and the box's keys move into its TPM.
  The directory's log items (names, public keys, passkey ids) pass through
  the relay too: encrypt them to the boxes' TPM keys (docs/dev/control-plane.md).

## License

Copyright (C) 2026 David Bascom

This program is free software: you can redistribute it and/or modify it under
the terms of the GNU Affero General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option) any
later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See the GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License along
with this program. If not, see <https://www.gnu.org/licenses/>.
