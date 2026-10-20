# The nix cache keys a box believes (storage/cache.nix): the laptop's,
# which signs releases, and the box's own, from fleet/cache-keys.pub (one
# "name:key" a line). Never another box's (nix/tests/cache-trust.nix).
{
  lib,
  host,
  listed,
}:
let
  all = lib.filter (k: k != "") (lib.splitString "\n" listed);
  named = n: lib.filter (k: lib.hasPrefix "dd-cache-${n}-" k) all;
in
named "laptop" ++ named host
