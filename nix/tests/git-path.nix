# The Git pages' early ask, under node: an address's owner and name are
# encoded into the forge's API path and never step out of it.
{ pkgs, ... }:
pkgs.runCommandLocal "git-path"
  {
    nativeBuildInputs = [ pkgs.nodejs ];
  }
  ''
    cp ${../../servers/forge/web/git-path.js} git-path.mjs
    cp ${./git-path/git-path.test.mjs} test.mjs
    node test.mjs
    touch $out
  ''
