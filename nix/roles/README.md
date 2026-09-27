# Roles

A box is its hardware plus a list of roles from `fleet/boxes.json`. A role is
a bundle of modules that have to move together, with the secrets those
modules read declared beside them. `flake.nix` builds one `nixosConfiguration`
per box from its entry; `hosts/<box>/hardware.nix` holds only what is true of
that machine (disks, network, lid).

    core      every box: identity, directory replica, metrics, tailnet, ssh, admin
    gateway   nginx, certificates, the browser login: a box with a public name
    storage   a node of the garage cluster; the box's own backup key
    photos    ente
    media     Movies & TV and Files tiles, uploads, per-user directories
    llm       llama-cpp and its gateway
    forge     forgejo, its runner, the github mirror
    observe   grafana, alerting, mail, database dumps

Every box backs itself up (nix/modules/storage/backup.nix): restic, encrypted on the
box, into its own bucket in the garage cluster, which keeps two copies. A
role adds the paths its state lives in with `dd.backup.paths`. A box's
datasets are declared in its hardware file with `dd.zfs.datasets`.

No box is trusted. Roles that read people's plaintext register with
`dd.box.plaintext`; the box publishes that list as a metric so the services
still to fix stay in view (nix/modules/box/box.nix). Nothing refuses anything.
