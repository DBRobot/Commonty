# What playing films needs of a box (types: nix/lib/needs-type.nix). The
# libraries themselves are part of the gate (dd-verify); their files are
# the storage pool's.
{
  dd-transcode = {
    what = "plays a film a member's device cannot, decrypting it in memory only";
    base = {
      # four sessions of 256M decrypted pieces each, and each one's twelve
      # minutes of segments (main.rs: BEHIND and AHEAD)
      requires.memory.fixed = "4G";
      faster = {
        # any maker's GPU that encodes H.264 (VAAPI): Intel's, AMD's
        gpu = {
          api = "vaapi";
          encode = [ "H264Main" ];
        };
        threads = 4; # without one, x264 on every core
      };
      # node1, 2026-10-04: idle
      uses = {
        memory = "3M";
        storage = "0";
      };
      network = {
        outbound = [ "s3.<domain>" ]; # the library, by range
        inbound = "localhost"; # files.<domain>'s nginx in front
      };
      depends = {
        nginx.local = true;
        dd-verify.local = true;
      };
      privacy = "plaintext"; # a film, in memory while it plays; never on disk
    };
  };
}
