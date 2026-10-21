# The shape of a service's needs (nix/modules/<area>/needs.nix): what it
# requires of a box, what makes it faster, what has to share its box, and
# what it can read. Typed, so a needs file that says something this does not
# know fails to evaluate. Tuning is not here: that is each module's settings.
{ lib }:
let
  t = lib.types;
  # sizes are binary: 1G is 1024^3 bytes, as du and systemd count them
  size = t.strMatching "[0-9]+(\\.[0-9]+)?[KMGT]";
  size0 = t.strMatching "0|[0-9]+(\\.[0-9]+)?[KMGT]";
  want = t.submodule {
    options = {
      arch = lib.mkOption {
        type = t.listOf (
          t.enum [
            "x86_64"
            "aarch64"
          ]
        );
        default = [ ];
        description = "empty: any architecture we build for";
      };
      gpu = lib.mkOption {
        type = t.nullOr (
          t.submodule {
            options = {
              api = lib.mkOption {
                type = t.enum [
                  "vulkan"
                  "vaapi"
                ];
              };
              encode = lib.mkOption {
                type = t.listOf t.str;
                default = [ ];
                description = "video profiles it must encode, as the report names them (H264Main)";
              };
              memory = lib.mkOption {
                type = t.nullOr size;
                default = null;
                description = "memory of its own (a graphics card's); null: shared is fine";
              };
            };
          }
        );
        default = null;
      };
      memory = lib.mkOption {
        type = t.nullOr (
          t.submodule {
            options = {
              fixed = lib.mkOption {
                type = t.nullOr size;
                default = null;
                description = "what it holds regardless of use (a model's weights)";
              };
              perKContext = lib.mkOption {
                type = t.nullOr size;
                default = null;
                description = "more per 1000 tokens of context; null: not measured yet";
              };
              locked = lib.mkOption {
                type = t.bool;
                default = false;
                description = "held in RAM (mlock): swap does not count";
              };
            };
          }
        );
        default = null;
      };
      disk = lib.mkOption {
        type = t.nullOr (
          t.submodule {
            options = {
              kind = lib.mkOption {
                type = t.enum [
                  "ssd"
                  "any"
                ];
                default = "any";
              };
              size = lib.mkOption {
                type = t.nullOr room;
                default = null;
                description = "its own space on this box";
              };
            };
          }
        );
        default = null;
      };
      place = lib.mkOption {
        type = t.enum [
          "any"
          "house"
        ];
        default = "any";
        description = "house: the box sits on a household's own network, with its fixed address and household in box.json";
      };
      threads = lib.mkOption {
        type = t.ints.unsigned;
        default = 0;
        description = "at least this many CPU threads";
      };
      windowsLicense = lib.mkOption {
        type = t.bool;
        default = false;
        description = "a Windows license in the box's firmware (its report's firmware.windowsKey), for a service that runs Windows";
      };
      kvm = lib.mkOption {
        type = t.bool;
        default = false;
      };
      uefi = lib.mkOption {
        type = t.bool;
        default = false;
        description = "boots by UEFI (its report's firmware.uefi): the signed boot the disk unlock's TPM policy measures";
      };
      wiredInitrd = lib.mkOption {
        type = t.bool;
        default = false;
        description = "a wired network the initrd has a driver for (its report's nics), to unlock the disks over the network when the TPM cannot";
      };
      tpm = lib.mkOption {
        type = t.bool;
        default = false;
        description = "a TPM 2.0 with P-256 keys (its report's tpm), for what signs as the box or unseals with it";
      };
      nestedKvm = lib.mkOption {
        type = t.bool;
        default = false;
      };
    };
  };
  # where it talks, which is not hardware but still decides where it can
  # go (a box on a filtered line) and, later, what its firewall lets out
  network = t.submodule {
    options = {
      outbound = lib.mkOption {
        type = t.either (t.enum [
          "none"
          "internet"
        ]) (t.listOf t.str);
        description = "none; internet (no fixed hosts, like search); or the hosts it reaches";
      };
      inbound = lib.mkOption {
        type = t.enum [
          "none"
          "localhost"
          "lan"
          "network"
          "public"
        ];
        description = "who reaches it: nobody (it only calls out); only this box; the household's own network (and its owner's devices); the Commonty network; anyone";
      };
    };
  };
  # the least space it needs on a box: a floor that makes sense for the
  # service, not what it holds today (that is uses.storage). Data it keeps
  # in another service (a bucket, a database) is that service's floor.
  room = t.submodule {
    options = {
      min = lib.mkOption {
        type = size0;
        default = "0";
      };
    };
  };
  # another service it needs: on this box (reached on localhost or a
  # socket) or anywhere
  dependency = t.submodule {
    options = {
      local = lib.mkOption { type = t.bool; };
    };
  };
  # what it costs to run, measured where it runs (systemd's peak, du of its
  # state); every service says, so placement can pack boxes
  uses = t.submodule {
    options = {
      memory = lib.mkOption { type = size0; };
      storage = lib.mkOption { type = size0; };
      estimated = lib.mkOption {
        type = t.bool;
        default = false;
        description = "true where it runs nowhere yet to be measured";
      };
    };
  };
  # a variant adds to the base; only the base must say everything
  part =
    full:
    t.submodule {
      options = {
        uses = lib.mkOption (
          { type = if full then uses else t.nullOr uses; } // lib.optionalAttrs (!full) { default = null; }
        );
        network = lib.mkOption (
          {
            type = if full then network else t.nullOr network;
          }
          // lib.optionalAttrs (!full) { default = null; }
        );
        requires = lib.mkOption {
          type = want;
          default = { };
          description = "it does not run without these";
        };
        faster = lib.mkOption {
          type = want;
          default = { };
          description = "it runs without these, slower";
        };
        depends = lib.mkOption (
          {
            type = t.attrsOf dependency;
            description = "the services it needs, by name; empty if none";
          }
          // lib.optionalAttrs (!full) { default = { }; }
        );
        privacy = lib.mkOption {
          type = t.nullOr (
            t.enum [
              "none"
              "metadata"
              "plaintext"
            ]
          );
          default = null;
          description = "what of people's data it can read; null: as the base says";
        };
      };
    };
in
t.attrsOf (
  t.submodule {
    options = {
      what = lib.mkOption { type = t.str; };
      units = lib.mkOption {
        type = t.listOf t.str;
        default = [ ];
        description = "the systemd services and timers it is made of, besides one named after it; one ending in * stands for every name that starts so (a template's instances, a name made at build time). Every unit our modules define belongs to some service (tests/needs-units.nix)";
      };
      servesCode = lib.mkOption {
        type = t.bool;
        default = false;
        description = "the box hands people the code that handles their secrets (a page a master password is typed into): whoever has root there could change it. fleet/policy.json can keep such a service on the fleet owner's own hardware";
      };
      when = lib.mkOption {
        type = t.nullOr t.str;
        default = null;
        description = "a fleet/policy.json setting that must be on (true, or a list with something in it) for the fleet to want it at all: the fleet's own CI only where the fleet develops Commonty (develops)";
      };
      everyBox = lib.mkOption {
        type = t.bool;
        default = false;
        description = "runs on every box, whatever its roles: what makes a box one of the fleet (placement does not choose it)";
      };
      placed = lib.mkOption {
        type = t.bool;
        default = false;
        description = "the control plane switches it on and off: its units start only where the plan puts it, among the boxes that carry it (box/placed.nix); a role still decides where it is built in. Every service that does not run on every box is placed, or a writer, but for one with no unit of its own. A writer's units name every service unit it is, its own name included if that is one; for the rest, every unit its name and units claim on the box is switched";
      };
      lease = lib.mkOption {
        type = t.bool;
        default = false;
        description = "one writer at a time: placed, and it runs only while its box holds the control plane's lease svc/<name>, which the boxes able to run it may hold (cmd/dd-control/placed.go)";
      };
      moves = lib.mkOption {
        type = t.enum [
          "better"
          "on-failure"
        ];
        default = "better";
        description = "better: it moves to a box that suits it better by the damping margin; on-failure: it stays on the box running it while that box is alive, even one it no longer fits (the plan's doesNotFit says so), and a new box is chosen only when that one is gone (servers/system/control/place)";
      };
      hosts = lib.mkOption {
        type = t.listOf t.str;
        default = [ ];
        description = "the names under the fleet's domain it answers at (photos, for photos.<domain>): with a lease, the DNS Worker points them at the box holding it";
      };
      buckets = lib.mkOption {
        type = t.attrsOf (
          t.submodule {
            options.copies = lib.mkOption {
              type = t.ints.positive;
              default = 2;
              description = "in how many boxes' stores it is kept, the one it is written on included";
            };
          }
        );
        default = { };
        description = "the buckets of the box's own store it writes (storage/store.nix): kept where the plan says, on boxes with room for them, and where it runs is one of them";
      };
      perHouse = lib.mkOption {
        type = t.nullOr t.ints.positive;
        default = null;
        description = "at most this many in one house (a house's DNS is one box)";
      };
      trust = lib.mkOption {
        type = t.enum [
          "owner"
          "household"
          "any"
        ];
        default = "any";
        description = "whose boxes may run it: the fleet owner's only (it holds the keys to everything, or hands people the code that holds theirs); the owner's or a box of a household it serves (it reads that household's own data); any box";
      };
      traffic = lib.mkOption {
        type = t.submodule {
          options = {
            class = lib.mkOption {
              type = t.enum [
                "high"
                "medium"
                "low"
              ];
              default = "low";
              description = "how much of a line it uses: high never goes on a metered line; medium is scored down there";
            };
            upMbps = lib.mkOption {
              type = t.ints.unsigned;
              default = 0;
              description = "the upload it wants to serve well; less still runs, scored down";
            };
            downMbps = lib.mkOption {
              type = t.ints.unsigned;
              default = 0;
              description = "the download it wants";
            };
          };
        };
        default = { };
      };
      alwaysOn = lib.mkOption {
        type = t.bool;
        default = false;
        description = "people expect it to answer at any hour: a box's uptime and power weigh in";
      };
      spread = lib.mkOption {
        type = t.bool;
        default = false;
        description = "on as many boxes, in as many houses, as can hold it (the storage pool), not a set number";
      };
      grows = lib.mkOption {
        type = t.listOf (
          t.enum [
            "photos"
            "libraries"
            "code"
            "passwords"
          ]
        );
        default = [ ];
        description = "what its size follows: the members' usage of these kinds (the fleet's count, storage/view.nix); uses.storage is then only its floor";
      };
      instances = lib.mkOption {
        type = t.nullOr t.ints.positive;
        default = null;
        description = "at most this many running at once (a database's single writer is 1); null: any";
      };
      base = lib.mkOption { type = part true; };
      variants = lib.mkOption {
        type = t.attrsOf (part false);
        default = { };
        description = "what it can run, each with its own needs on top of the base (a model, a game)";
      };
    };
  }
)
