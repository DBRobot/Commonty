# Who Tang answers, without a VM: the house a box names, and on a box with no
# fixed address (the installer's, house.address empty) the private ranges,
# never an empty -s that would stop its firewall starting. home-controller
# has the real socket.
{ pkgs, ... }:
let
  rules =
    house:
    (pkgs.nixos {
      imports = [ ../modules/smarthome/tang.nix ];
      dd.tang = {
        enable = true;
        inherit house;
      };
      boot.isContainer = true;
      system.stateVersion = "25.05";
    }).config;
  fixed = rules "10.77.0.5/24";
  dhcp = rules "";
in
pkgs.runCommand "tang-house"
  {
    fixed = fixed.networking.firewall.extraCommands;
    dhcp = dhcp.networking.firewall.extraCommands;
    allowFixed = toString fixed.services.tang.ipAddressAllow;
    allowDhcp = toString dhcp.services.tang.ipAddressAllow;
    nativeBuildInputs = [ pkgs.gnugrep ];
  }
  ''
    printf '%s\n' "$fixed" | grep -q -- '-I nixos-fw -p tcp --dport 7654 -s 10.77.0.5/24 -j'
    [ "$(printf '%s\n' "$fixed" | grep -c -- ' -I nixos-fw')" = 1 ]
    [ "$allowFixed" = "10.77.0.5/24 127.0.0.1/32" ]
    for a in 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16; do
      printf '%s\n' "$dhcp" | grep -q -- "-I nixos-fw -p tcp --dport 7654 -s $a -j"
    done
    if printf '%s\n' "$dhcp" | grep -q -- '-s  '; then
      echo "an empty source in the firewall's rules"
      exit 1
    fi
    [ "$allowDhcp" = "10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 127.0.0.1/32" ]
    touch $out
  ''
