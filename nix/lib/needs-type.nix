# The shape of a service's needs (nix/modules/<area>/needs.nix): what it
# requires of a box, what makes it faster, what has to share its box, and
# what it can read. Typed, so a needs file that says something this does not
# know fails to evaluate. Tuning is not here: that is each module's settings.
{ lib }:
let
  t = lib.types;
  size = t.strMatching "[0-9]+(\\.[0-9]+)?[KMGT]";
  want = t.submodule {
    options = {
      arch = lib.mkOption {
        type = t.listOf (t.enum [ "x86_64" "aarch64" ]);
        default = [ ];
        description = "empty: any architecture we build for";
      };
      gpu = lib.mkOption {
        type = t.nullOr (t.submodule {
          options = {
            api = lib.mkOption { type = t.enum [ "vulkan" "vaapi" ]; };
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
        });
        default = null;
      };
      memory = lib.mkOption {
        type = t.nullOr (t.submodule {
          options = {
            fixed = lib.mkOption { type = t.nullOr size; default = null; description = "what it holds regardless of use (a model's weights)"; };
            perKContext = lib.mkOption { type = t.nullOr size; default = null; description = "more per 1000 tokens of context; null: not measured yet"; };
            locked = lib.mkOption { type = t.bool; default = false; description = "held in RAM (mlock): swap does not count"; };
          };
        });
        default = null;
      };
      disk = lib.mkOption {
        type = t.nullOr (t.submodule {
          options = {
            kind = lib.mkOption { type = t.enum [ "ssd" "any" ]; default = "any"; };
            size = lib.mkOption { type = t.nullOr size; default = null; };
          };
        });
        default = null;
      };
      kvm = lib.mkOption { type = t.bool; default = false; };
      nestedKvm = lib.mkOption { type = t.bool; default = false; };
      outbound = lib.mkOption {
        type = t.listOf t.str;
        default = [ ];
        description = "hosts it has to reach on the internet";
      };
    };
  };
  part = t.submodule {
    options = {
      requires = lib.mkOption { type = want; default = { }; description = "it does not run without these"; };
      faster = lib.mkOption { type = want; default = { }; description = "it runs without these, slower"; };
      "with" = lib.mkOption {
        type = t.listOf t.str;
        default = [ ];
        description = "services that must be on the same box (it reaches them on localhost)";
      };
      privacy = lib.mkOption {
        type = t.nullOr (t.enum [ "none" "metadata" "plaintext" ]);
        default = null;
        description = "what of people's data it can read; null: as the base says";
      };
    };
  };
in
t.attrsOf (t.submodule {
  options = {
    what = lib.mkOption { type = t.str; };
    base = lib.mkOption { type = part; default = { }; };
    variants = lib.mkOption {
      type = t.attrsOf part;
      default = { };
      description = "what it can run, each with its own needs on top of the base (a model, a game)";
    };
  };
})
