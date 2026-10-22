# The smallest thing that is still one of our boxes: the modules under test
# and nothing that needs real hardware, real dns or a real certificate. A
# test boots two or three of these in a few seconds; the real hosts import
# the same modules, so what passes here is what runs there.
{ lib, ... }:
{
  imports = [
    ../modules/box/domain.nix
    ../modules/box/box.nix
    ../modules/gate/verify.nix
    ../modules/observe/metrics.nix
  ];
  dd.domain = "test.invalid";
  dd.repo = "tester/commonty";
  dd.verify.role = "directory";
  # a test chooses its own failover mode, never the fleet's switch
  # (fleet/policy.json): off, a service runs from the test's own secrets as
  # it does on a box built before failover; the tests of failover itself
  # (takeover, the follow tests, vault, placed) turn it on
  dd.failover = lib.mkDefault false;
  # the tests ask the directory on loopback, from the box itself
  dd.verify.names = [ "127.0.0.1" ];
  networking.firewall.enable = false; # the test network is the only network
}
