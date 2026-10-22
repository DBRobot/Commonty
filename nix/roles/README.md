# Bundles

A bundle is modules that have to move together, with the secrets those
modules read declared beside them. No box is given one by hand: a box is
built with every bundle of which it can run a service, whose secrets its own
sops file holds, and whose companions it is built with too
(nix/lib/fleet-placement.nix, from its folder in `fleet/boxes/<box>/`: what
the box found on itself and what its owner chose). `bundles.json` says what
each brings (`services`), what it reads from the box's file whatever else
(`secrets`), and what must share its box (`with`). The control plane
decides where each placed service runs.

    core      every box: identity, directory replica, metrics, the fleet's network, ssh, admin
    gateway   certificates, the public front door (tunnel, bare domain), headscale
    photos    ente
    media     Movies & TV and Files tiles, uploads, per-user directories
    library   members' encrypted libraries and their locked mail behind the gate, and the transcoder
    llm       llama-cpp, its Chat page and web search
    game      game servers a member starts, each in a guest of its own
    forge     forgejo
    runner    a CI runner for the forge; any box can run one
    observe   our Metrics page over Thanos, alerting, mail
    vault     Passwords: vaultwarden and the breach list
    adblock   Pi-hole and Tang for the house the box is in
    compute   members' remote computers

Every box backs itself up (nix/modules/storage/backup.nix): restic, encrypted on the
box, into its own bucket in its own store, copied where the plan finds room;
core keeps its pool's datasets (snapshots and scrubs). A bundle adds the paths its state lives in with `dd.backup.paths`. A box's
datasets are declared in its `fleet/boxes/<box>/box.json` (`datasets`),
which modules/box/host.nix turns into `dd.zfs.datasets`.

No box is trusted. Bundles that read people's plaintext register with
`dd.box.plaintext`; the box publishes that list as a metric so the services
still to fix stay in view (nix/modules/box/box.nix). Nothing refuses anything.
