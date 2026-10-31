# What a joined box removes of a former owner (box/forget-owner.sh): theirs and
# nobody else's, nothing for a member it never knew, and not what still runs.
{ pkgs, ... }:
pkgs.runCommand "forget-owner"
  {
    nativeBuildInputs = [
      pkgs.bash
      pkgs.coreutils
      pkgs.jq
    ];
  }
  ''
    run() { FORGET_ROOT=$PWD/r bash ${../modules/box/forget-owner.sh} "$@"; }
    fresh() {
      chmod -R u+w r 2>/dev/null; rm -rf r
      mkdir -p r/var/lib/dd-control/owners-seen r/var/lib/dd-computer-owners r/run/dd-placed
      mkdir -p r/var/lib/dd-games/records r/var/lib/dd-games/worlds/ann r/var/lib/dd-games/worlds/bea
      for pair in aaaaaaaaaaaaaaaa:ann bbbbbbbbbbbbbbbb:bea; do
        id=''${pair%:*}; who=''${pair#*:}
        echo "$who" > r/var/lib/dd-computer-owners/$id
        mkdir -p r/var/lib/private/dd-computer/$id
        touch r/var/lib/private/dd-computer/$id/home.qcow2
        echo "{\"owner\":\"$who\"}" > r/var/lib/dd-games/records/$who-1.json
        mkdir -p r/var/lib/dd-games/instances/$who-1
      done
      touch r/var/lib/dd-control/owners-seen/ann
    }

    fresh
    run ann
    [ ! -e r/var/lib/private/dd-computer/aaaaaaaaaaaaaaaa ] && [ ! -e r/var/lib/dd-computer-owners/aaaaaaaaaaaaaaaa ]
    [ ! -e r/var/lib/dd-games/records/ann-1.json ] && [ ! -e r/var/lib/dd-games/instances/ann-1 ] && [ ! -e r/var/lib/dd-games/worlds/ann ]
    [ -e r/var/lib/private/dd-computer/bbbbbbbbbbbbbbbb/home.qcow2 ] && [ -e r/var/lib/dd-computer-owners/bbbbbbbbbbbbbbbb ]
    [ -e r/var/lib/dd-games/records/bea-1.json ] && [ -e r/var/lib/dd-games/instances/bea-1 ] && [ -e r/var/lib/dd-games/worlds/bea ]
    [ ! -e r/var/lib/dd-control/owners-seen/ann ]
    # and once done, a second word does nothing
    if run ann; then echo "ran with no record of them"; exit 1; fi

    # a member this box never knew: refused, nothing touched
    fresh
    if run bea; then echo "removed a member it never knew"; exit 1; fi
    [ -e r/var/lib/private/dd-computer/bbbbbbbbbbbbbbbb/home.qcow2 ] && [ -e r/var/lib/dd-games/worlds/bea ]

    # names that would walk out of the folders
    fresh
    for bad in "" ".." "../ann" "a/b" "*" "-rf"; do
      if run "$bad"; then echo "took the name '$bad'"; exit 1; fi
    done
    [ -e r/var/lib/private/dd-computer/aaaaaaaaaaaaaaaa/home.qcow2 ]

    # what still runs stays, and it says so, to be tried again
    fresh
    touch r/run/dd-placed/computer-aaaaaaaaaaaaaaaa r/run/dd-placed/dd-games
    if run ann; then echo "said it was done with a computer running"; exit 1; fi
    [ -e r/var/lib/private/dd-computer/aaaaaaaaaaaaaaaa/home.qcow2 ] && [ -e r/var/lib/dd-games/worlds/ann ]
    [ -e r/var/lib/dd-control/owners-seen/ann ]

    touch $out
  ''
