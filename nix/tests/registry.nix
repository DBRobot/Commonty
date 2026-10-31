# Every test CI knows, by name. Adding a name here adds that test, and is
# not a change to CI: the chooser runs a name new to this file once, and
# nothing else for it (nix/tests/ci-select/ciselect.py, registered).
{
  # boxes booted as VMs and driven through their failure cases
  # (nix/tests/<name>.nix and <name>.py), one vm_tests job each
  vm = [
    "acme"
    "console"
    "directory"
    "metrics"
    "metrics-ui"
    "backup"
    "release"
    "switch-remote"
    "thanos"
    "members-runner"
    "games"
    "computers"
    "forge"
    "transcode"
    "network"
    "library"
    "pgrestore"
    "pg-follow"
    "store"
    "takeover"
    "forge-follow"
    "net-follow"
    "join-install"
    "joined-box"
    "joined-serves"
    "joined-games"
    "photos"
    "llm"
    "vault"
    "adblock"
    "unlock"
    "leaving"
    "home-controller"
    "waits"
    "switch-race"
    "work-memory"
    "runner-restart"
    "cache-upload"
    "cache-front"
    # each web service's one main thing, in a browser, with the real services
    "site-journeys"
    "rollout"
    "placed"
    "wired-watch"
    "net-detect"
  ];
  # checks that are a file of their own, nix/tests/<name>.nix (or
  # <name>/default.nix), given the flake's usual args
  files = [
    "plaintext-label"
    "calendar-js"
    # the Git pages' early ask stays inside /repos/<owner>/<name>
    "git-path"
    "boxes"
    "box-reports"
    # builds and CI held to a share of each box's memory, together
    "work-memory-sizes"
    # what a box finds of its network links, and the config made of it
    "net-report"
    # the wired watchdog's fallback, choice by choice
    "wired-watch-logic"
    # a box's room for computers, from its services' measured memory
    "computer-facts-logic"
    # anything deleted is gone from every box and backup within 90 days
    "retention"
    # every unit our modules define is in some service's needs, and no
    # timer of ours counts from boot
    "needs-units"
    # every box can carry what its roles bring, by the needs; and the
    # roles say truly what they bring (nix/lib/capability.nix)
    "placement"
    # capability on every kind of computer, held to answers that make sense
    "placement-fleet"
    "nginx"
    # the mail Worker in a browser, against a fake Cloudflare
    "mail-worker"
    # every link the site's pages show, followed in a browser against the
    # gateway box's own nginx and gate, and the app's pages' links read
    "site-links"
    "safe-rd"
    # how many jobs a runner takes, from the box as it is
    "runner-capacity"
    # a box believes its own cache key and the laptop's, never another box's
    "cache-trust"
    # garage-setup reads garage out whole; a closed pipe aborts garage
    "garage-setup-pipe"
    "sso"
    # every bar link and home tile stays in its tab, on a box's pages and the app's
    "same-tab"
    "dns-worker"
    # the control plane: relay, witness and three validators, offline
    "control"
    # the relay answers without a witness that cannot start
    "control-relay"
    # a CI job a restart killed runs again by itself (ci-resume.py)
    "ci-resume"
  ];
  # the evaluation-only checks flake_check builds: seconds each, no VM
  eval = [
    "plaintext-label"
    "boxes"
    "box-reports"
    "ci"
    "ci-select"
    "stylesheet"
    "cargo-nix"
    "nginx"
    "usage-facts"
    "fleet-defaults"
    "retention"
    "terms-version"
    "needs-units"
    "placement"
    "safe-rd"
    "runner-capacity"
    "cache-trust"
    "placement-fleet"
    "pdf-editor"
    "calendar-js"
    "git-path"
    # no VM, seconds each: run by flake_check like the rest of this list
    "work-memory-sizes"
    "net-report"
    "wired-watch-logic"
    "garage-setup-pipe"
    "computer-facts-logic"
    "ci-resume"
  ];
}
