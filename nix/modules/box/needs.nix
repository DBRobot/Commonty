# What every box runs to be one of the fleet (types: nix/lib/needs-type.nix)
{
  dd-control = {
    what = "the control plane: heartbeats, leases and the directory, agreed by the voting boxes and a witness";
    base = {
      # signs with the key in the TPM; its disk unlock already needs one
      requires.tpm = true;
      # node1, node2, 2026-10-04: peak; its state (the agreed log)
      uses = {
        memory = "21M";
        storage = "227K";
      };
      network = {
        outbound = [ "control" ]; # control.<domain>, the relay at Cloudflare
        inbound = "localhost"; # the gate hands in directory items on a socket
      };
      depends = { };
      privacy = "metadata"; # names and public keys; encrypting them is a README TODO
    };
  };

  dd-agent = {
    what = "moves the box to the signed release, and back if the box comes up worse";
    base = {
      uses = {
        memory = "70M";
        storage = "0"; # closures are the nix store's
      };
      network = {
        outbound = [
          "git"
          "s3"
        ]; # the release on the forge, closures from the cache
        inbound = "none";
      };
      depends = { };
      privacy = "none";
    };
  };
}
