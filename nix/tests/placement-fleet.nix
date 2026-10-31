# Capability (nix/lib/capability.nix) on a fleet of every kind of computer
# someone might offer (placement-fleet/fleet.json: a mini PC, NASes, Xeon
# servers, a gaming PC, laptops, a Pi, a VPS, a thin client, an old office
# PC), held to answers a person would give. A change to the needs or the
# rules that makes a placement stop making sense fails here, naming it.
{
  pkgs,
  lib,
  ...
}:
let
  sim = lib.importJSON ./placement-fleet/fleet.json;
  cap = import ../lib/capability.nix {
    inherit lib;
    needs = import ../lib/needs.nix { inherit lib; };
    policy = lib.importJSON ../../fleet/policy.json;
    boxes = lib.mapAttrs (
      n: b:
      b
      // {
        site = n;
        region = n;
      }
    ) sim;
  };
  joins = b: cap.boxes.${b}.joins;
  can = b: s: cap.boxes.${b}.can ? ${s};
  why = b: s: lib.concatStringsSep "; " (cap.boxes.${b}.cannot.${s} or [ ]);
  joinWhy = b: lib.concatStringsSep "; " cap.boxes.${b}.why;
  best = s: (builtins.head cap.services.${s}).box;
  rank = s: map (x: x.box) cap.services.${s};
  before =
    s: a: b:
    lib.lists.findFirstIndex (x: x == a) 999 (rank s)
    < lib.lists.findFirstIndex (x: x == b) 999 (rank s);

  expect = [
    # a box unlocks its disks and signs what it says with a TPM 2.0, and
    # unlocks over the wire at boot
    (!(joins "rpi5") && lib.hasInfix "TPM" (joinWhy "rpi5"))
    (!(joins "synology") && lib.hasInfix "TPM" (joinWhy "synology"))
    (!(joins "vps") && lib.hasInfix "TPM" (joinWhy "vps"))
    (!(joins "xeon-r720") && lib.hasInfix "TPM" (joinWhy "xeon-r720"))
    (!(joins "sister-laptop") && lib.hasInfix "wired NIC" (joinWhy "sister-laptop"))
    # the owner's boxes can run everything their hardware allows
    # but the games, whose worlds want a dataset of their own (box.json)
    (joins "david-laptop" && builtins.attrNames cap.boxes."david-laptop".cannot == [ "dd-games" ])
    (lib.hasInfix "dataset" (why "david-laptop" "dd-games"))
    # (and search, which answers under the chat's name, beside it)
    (
      joins "n100-minipc"
      &&
        builtins.attrNames cap.boxes."n100-minipc".cannot == [
          "dd-games"
          "llama-swap"
          "searxng"
        ]
    )
    (lib.hasInfix "memory" (why "n100-minipc" "llama-swap"))
    (
      # (and the house's DNS and Tang: no household says it is theirs)
      builtins.attrNames cap.boxes."xeon-r740".cannot == [
        "dd-games"
        "llama-swap"
        "pihole"
        "searxng"
        "tang"
      ]
    )
    (lib.hasInfix "GPU" (why "xeon-r740" "llama-swap"))
    # what holds keys stays on the owner's boxes; a model does not
    (!(can "gaming-pc" "headscale") && lib.hasInfix "only on" (why "gaming-pc" "headscale"))
    (!(can "gaming-pc" "vaultwarden"))
    # a household's own DNS on that household's box
    (can "diy-nas" "pihole" && can "office-desktop" "pihole" && can "thin-client" "pihole")
    # postgres wants an SSD; a NAS of spinning disks has none
    (!(can "diy-nas" "postgresql") && lib.hasInfix "SSD" (why "diy-nas" "postgresql"))
    (can "n100-minipc" "postgresql")
    # what is kept off other people's boxes stays off
    (!(can "office-desktop" "postgresql") && lib.hasInfix "only on" (why "office-desktop" "postgresql"))
    # a 64G eMMC holds no nix cache
    (!(can "thin-client" "nix-cache") && lib.hasInfix "storage" (why "thin-client" "nix-cache"))
    # members' CI anywhere with the threads
    (can "gaming-pc" "members-runner" && can "diy-nas" "members-runner")
    # a box's certificate is its own names, asked of the DNS Worker, so
    # what sits behind nginx may run on anyone's box: a game server on a
    # gaming PC
    (can "gaming-pc" "dd-games")
    # rankings: the biggest pool for storage, a video encoder for
    # transcoding, mains over battery for what must always answer
    (before "dd-transcode" "n100-minipc" "xeon-r740")
    (before "pihole" "office-desktop" "david-laptop")
    (best "members-runner" == "xeon-r740")
  ];
  names = [
    "rpi5 cannot join: no TPM"
    "synology cannot join: no TPM"
    "vps cannot join: no TPM"
    "xeon-r720 cannot join: TPM 1.2"
    "sister-laptop cannot join: no wired NIC at boot"
    "david-laptop can run everything but the games, without their dataset"
    "david-laptop: games for want of a dataset"
    "n100-minipc can run all but llama-swap and its search"
    "n100-minipc: llama-swap for memory"
    "xeon-r740 can run all but llama-swap and its search"
    "xeon-r740: llama-swap for want of a GPU"
    "gaming-pc: headscale is the owner's"
    "gaming-pc: no vault"
    "pihole on households' boxes"
    "diy-nas: no postgres without an SSD"
    "n100-minipc: postgres"
    "office-desktop: no postgres, it holds the vault's records"
    "thin-client: no nix cache"
    "members-runner on helpers' boxes"
    "gaming-pc: no games until certificates move"
    "transcode: an encoder first"
    "pihole: mains before battery"
    "members-runner best on the most threads"
  ];
  failed = lib.concatLists (lib.zipListsWith (ok: n: lib.optional (!ok) n) expect names);
in
assert builtins.length expect == builtins.length names;
if failed == [ ] then
  pkgs.writeText "placement-fleet.json" (builtins.toJSON cap)
else
  throw ("\nplacement no longer makes sense:\n" + lib.concatStringsSep "\n" failed)
