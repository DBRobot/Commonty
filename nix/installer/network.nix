# dd-installer-net: the box installer on the fleet's network for its
# disk's backup alone (network.sh). The installer's, and the network test's.
{ pkgs, self }:
let
  sys = pkgs.stdenv.hostPlatform.system;
in
pkgs.writeShellApplication {
  name = "dd-installer-net";
  runtimeInputs = [
    self.packages.${sys}.agent
    self.packages.${sys}.netbridge
    pkgs.tailscale
    pkgs.age
    pkgs.jq
    pkgs.curl
    pkgs.coreutils
    pkgs.iproute2
  ];
  text = builtins.readFile ./network.sh;
}
