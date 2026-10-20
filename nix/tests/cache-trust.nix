# Each runner box believes the laptop's cache key and its own, never another
# box's (storage/cache.nix): read from every runner's built config, then
# tried on nix itself. Two boxes' keys sign a path each into one binary
# cache; a store trusting what node1's config would trust takes node1's
# path from it and refuses node2's.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  boxes = lib.importJSON ../../fleet/boxes.json;
  runners = lib.attrNames (lib.filterAttrs (_: b: builtins.elem "runner" (b.roles or [ ])) boxes);
  listed = lib.splitString "\n" (lib.fileContents ../../fleet/cache-keys.pub);
  nameOf = k: builtins.head (lib.splitString ":" k);
  problems = lib.concatMap (
    box:
    let
      have = self.nixosConfigurations.${box}.config.nix.settings.trusted-public-keys;
      own = lib.filter (k: lib.hasPrefix "dd-cache-${box}-" k) listed;
      others = lib.filter (
        k: lib.hasPrefix "dd-cache-" k && !(lib.hasPrefix "dd-cache-laptop-" k) && !(builtins.elem k own)
      ) listed;
    in
    lib.optional (own == [ ]) "${box}: no dd-cache-${box}-* key in fleet/cache-keys.pub"
    ++ map (k: "${box}: does not trust its own key ${nameOf k}") (
      lib.filter (k: !(builtins.elem k have)) own
    )
    ++ map (k: "${box}: trusts another box's key ${nameOf k}") (
      lib.filter (k: builtins.elem k have) others
    )
  ) runners;
  # the module's choice, on made-up keys: names only, the keys are made below
  chosen = map nameOf (
    import ../lib/cache-keys.nix {
      inherit lib;
      host = "node1";
      listed = "dd-cache-laptop-1:x\ndd-cache-node1-1:x\ndd-cache-node2-1:x\n";
    }
  );
in
if problems != [ ] then
  throw ("\n" + lib.concatStringsSep "\n" problems)
else
  pkgs.runCommand "cache-trust" { nativeBuildInputs = [ pkgs.nix ]; } ''
    export HOME=$TMPDIR NIX_CONFIG="experimental-features = nix-command
    sandbox = false
    build-users-group ="
    for n in dd-cache-laptop-1 dd-cache-node1-1 dd-cache-node2-1; do
      nix key generate-secret --key-name $n > $n.sec
      nix key convert-secret-to-public < $n.sec > $n.pub
    done
    trusted=""
    for n in ${toString chosen}; do trusted="$trusted $(cat $n.pub)"; done
    a="local?root=$TMPDIR/a"
    echo node1 > f1; echo node2 > f2
    p1=$(nix store add-file --store "$a" ./f1)
    p2=$(nix store add-file --store "$a" ./f2)
    nix store sign --store "$a" --key-file dd-cache-node1-1.sec "$p1"
    nix store sign --store "$a" --key-file dd-cache-node2-1.sec "$p2"
    nix copy --store "$a" --to "file://$TMPDIR/cache" "$p1" "$p2"
    # a file added this way is content-addressed, which nix takes unsigned;
    # a build's output is not, so the cache is made to say what a build's
    # would (the signature does not cover the CA line)
    sed -i '/^CA: /d' $TMPDIR/cache/*.narinfo
    get() {
      nix-store --store "local?root=$TMPDIR/b" --realise "$1" \
        --option substituters "file://$TMPDIR/cache" --option trusted-public-keys "$trusted"
    }
    get "$p1"
    if get "$p2" 2> refused; then
      echo "a path node2 signed was taken on node1's keys" >&2
      exit 1
    fi
    grep -q "not signed by any of the keys" refused || { cat refused >&2; exit 1; }
    echo "node1 takes its own, refuses node2's" > $out
  ''
