# A box's network hardware, from its report to its config: report/nics.sh
# on node1's and node2's links (net-report/run.sh), and what
# nix/lib/net-hardware.nix makes of node2's - and of a box with no Wi-Fi, a
# box with a pci adapter, and a report from before the new fields.
{ pkgs, lib, ... }:
let
  derive = import ../lib/net-hardware.nix;
  node2 = lib.importJSON ./net-report/node2.json;
  house = {
    node1 = "192.168.1.20";
  };
  on = hw: derive { inherit hw; houseBoxes = house; };
  has = s: ws: lib.any (lib.hasInfix s) ws;
  d2 = on node2;
  noWifi = on (node2 // { nics = lib.filter (n: !n.wireless) node2.nics; stack = null; });
  pciOnly = on {
    nics = [
      {
        name = "enp1s0";
        mac = "AA";
        driver = "igc";
        wireless = false;
        usb = false;
        initrd = false;
        bus = "pci";
        host = null;
      }
    ];
    stack = null;
  };
  # node2's hardware.json as it was on 2026-10-11, before bus and host
  old = on {
    nics = [
      {
        name = "enp3s0f4u1";
        mac = "00:E0:4C:3A:46:58";
        driver = "r8152";
        wireless = false;
        usb = true;
        initrd = true;
      }
    ];
  };
  checks = {
    "the cable's driver and its usb host in the initrd" = d2.initrdModules == [ "r8152" "xhci_pci" ];
    "the Wi-Fi adapter is the fallback's" = d2.wifi == "wlp2s0";
    "r8152 behind xhci is known to fail" = map (f: f.name) d2.flaky == [ "enp3s0f4u1" ];
    "the fallback's DNS on node1 is said" = has "asks node1 for names first" d2.warnings;
    "two warnings for node2" = lib.length d2.warnings == 2;
    "no Wi-Fi: no fallback, and said" = noWifi.wifi == null && has "no Wi-Fi to fall back to" noWifi.warnings;
    "a pci adapter: nothing known, nothing said" = pciOnly.warnings == [ ] && pciOnly.initrdModules == [ "igc" ];
    "an old report's usb adapter still warned of" = lib.length old.flaky == 1 && old.initrdModules == [ "r8152" "xhci_pci" ];
  };
  failed = lib.attrNames (lib.filterAttrs (_: ok: !ok) checks);
in
assert lib.assertMsg (failed == [ ]) "net-report: ${lib.concatStringsSep "; " failed}";
pkgs.runCommand "net-report"
  {
    nativeBuildInputs = [
      pkgs.jq
      pkgs.gawk
      pkgs.gnused
      pkgs.gnugrep
    ];
  }
  ''
    bash ${./net-report/run.sh} ${../modules/box/report/nics.sh} ${./net-report/node2.json}
    touch $out
  ''
