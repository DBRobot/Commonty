# fleet/boxes.json gives each box only roles its signed report and its
# owner's box.json can carry (fleet/roles.json, nix/lib/fit.nix)
{ pkgs, lib, ... }:
let
  problems = import ../lib/fit.nix { inherit lib; } {
    roles = lib.importJSON ../../fleet/roles.json;
    boxes = lib.importJSON ../../fleet/boxes.json;
    dir = ../../fleet/boxes;
  };
in
pkgs.runCommand "role-fit" { } (
  if problems == [ ] then
    "echo 'every box fits its roles' && touch $out"
  else
    "printf '%s\\n' ${lib.escapeShellArgs problems} >&2; exit 1"
)
