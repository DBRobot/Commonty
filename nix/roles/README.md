# Roles

A box is its hardware plus a list of roles from `fleet/boxes.json`. A role is
a bundle of modules that have to move together, with the secrets those
modules read declared beside them. `flake.nix` builds one `nixosConfiguration`
per box from its entry and its folder in `fleet/boxes/<box>/`, which is data,
not Nix: what the box found on itself and what its owner chose
(modules/box/host.nix).

    core      every box: identity, directory replica, metrics, the fleet's network, ssh, admin
    gateway   certificates, the public front door (tunnel, bare domain), headscale
    storage   a node of the garage cluster; the box's own backup key
    photos    ente
    media     Movies & TV and Files tiles, uploads, per-user directories
    library   members' encrypted libraries and their locked mail behind the gate, and the transcoder
    llm       llama-cpp, its Chat page and web search
    game      game servers a member starts, each in a guest of its own
    forge     forgejo and the github mirror
    runner    a CI runner for the forge; any box can run one
    observe   our Metrics page over Thanos, alerting, mail
    vault     Passwords: vaultwarden and the breach list
    adblock   Pi-hole for the house the box is in

Every box backs itself up (nix/modules/storage/backup.nix): restic, encrypted on the
box, into its own bucket in the garage cluster, which keeps two copies. A
role adds the paths its state lives in with `dd.backup.paths`. A box's
datasets are declared in its `fleet/boxes/<box>/box.json` (`datasets`),
which modules/box/host.nix turns into `dd.zfs.datasets`.

No box is trusted. Roles that read people's plaintext register with
`dd.box.plaintext`; the box publishes that list as a metric so the services
still to fix stay in view (nix/modules/box/box.nix). Nothing refuses anything.
