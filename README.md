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
      roles/           what a box runs, by role; a box lists its roles in fleet/
      tests/           VM tests: boxes booted and driven
    box/               Rust that runs on boxes: the agent, the gate, the games manager
    client/            Rust a person runs: the dd cli, its libraries, the browser wasm
    data/              the games catalogue and covers
    scripts/           deploy, install-box, restore-box
    secrets/           sops-encrypted values (safe to publish)

## Deploying

    nixos-rebuild switch --flake github:DBRobot/Home-Server#node1 --refresh

`--refresh` matters: nix caches flake tarballs for an hour and will otherwise
silently redeploy the previous revision.

## TODO

- **No box talks to another box.** A box is reached by people and by
  services through its front door, the way any client reaches it, and never
  through a channel that exists because the other end is also a box. Today
  three such channels remain, all over the owner's tailnet and fenced to the
  peer's address: garage RPC (3901), the directory pull (4181), and the
  thanos sidecar (10901). Each goes when the change it waits on lands.
- **One Garage per box, or per trust domain, not one cluster.** A Garage
  cluster shares one RPC secret and every member of it is an admin, so a box
  in someone else's house cannot be a peer. What replaces the cluster has to
  let contributed boxes *grow* the usable pool, not only hold copies:
  placement and replication across independent stores, each with its own
  credentials. Erasure coding is the eventual shape and nothing off the shelf
  fits (Garage has none by design, MinIO is archived, SeaweedFS shares a
  signing key across volume servers, Tahoe-LAFS fits the trust model but not
  S3). It stops mattering until there are five or six houses; below that,
  replication is optimal anyway.
- **Metrics without pulling.** Once each box's store is its own, the thanos
  sidecar channel goes too and a box's history reaches the fleet view only
  through the bucket.
- **An off-site copy.** Every copy of every member's data is in one building.
- **One vote per house in the control plane.** It starts with three voters:
  node1, node2 and the Cloudflare witness (docs/control-plane.md). That
  survives any one of them going down, but it is not Byzantine-safe (that
  takes four), and node1 and node2 share a house, so the house votes twice.
  As boxes go off site: each new house gets one validator and node2 stops
  voting; outside witnesses fill the count up to four in the meantime (a
  small VPS, or a member's always-on device in another place); and once
  there are four houses, the witnesses retire, the Cloudflare one last. A
  house that joins changes the validator list in a signed release, never on
  its own say.

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
