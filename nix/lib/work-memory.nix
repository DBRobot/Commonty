# What a box's builds and CI may hold of its memory, all of them together
# (modules/box/work-memory.nix): everything but a reserve for the box's own
# services and the kernel - a fifth of it, at least 3G, at most half.
# 14G: 11G of work; 62G: 50G.
{ lib }:
memoryBytes:
let
  gib = 1024 * 1024 * 1024;
  reserve = lib.min (memoryBytes / 2) (lib.max (3 * gib) (memoryBytes / 5));
in
{
  inherit reserve;
  slice = memoryBytes - reserve;
}
