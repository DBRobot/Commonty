# One box's directory across a real boot: a client publishes, admits a
# device, and the entry survives a restart. The list is members' on both
# boxes; once the release names a reader key (b), nobody reads a single
# entry without a credential either. Boxes share the directory through the
# control plane's agreed log (nix/tests/control); none pulls another.
{
  pkgs,
  self,
  ...
}:
{
  name = "directory";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 1024;
  defaults.virtualisation.cores = 2; # services start in parallel instead of queueing on one thread
  defaults.virtualisation.diskSize = 2048;
  nodes = {
    # no reader key yet: a single entry is anyone's, the list is not
    a = {
      imports = [ ./box.nix ];
      # the client asks it by this name: its tokens are for it
      dd.verify.names = [ "a" ];
    };
    # the release names the mail Worker's key: nothing without a credential
    b = {
      imports = [ ./box.nix ];
      dd.verify.names = [ "b" ];
      dd.verify.directoryReaders = [ "BQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQU=" ];
    };
    client = {
      environment.systemPackages = [ pkgs.curl ];
    };
  };
  scriptEnv = {
    dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
  };
}
