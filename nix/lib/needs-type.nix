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
      threads = lib.mkOption {
        type = t.ints.unsigned;
        default = 0;
        description = "at least this many CPU threads";
      };
      kvm = lib.mkOption {
        type = t.bool;
        default = false;
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
          "localhost"
          "tailnet"
          "public"
        ];
        description = "who reaches it: nobody (it only calls out); only this box; the Commonty network; anyone";
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
      servesCode = lib.mkOption {
        type = t.bool;
        default = false;
        description = "the box hands people the code that handles their secrets (a page a master password is typed into): whoever has root there could change it. fleet/policy.json can keep such a service on the fleet owner's own hardware";
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
