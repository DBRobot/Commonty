# The compute side of the libraries: a clip is chunked and encrypted the
# way a library holds it (rclone writes it; nix/tests/transcode-chunks.py
# makes its block keys and seals them to the box), served over plain http
# like the bucket would, and the box turns it into HLS with those keys
# alone, sealed to the key it made at start: never the data key, never the
# names' key. The session is wiped on request.
{ self, pkgs, ... }:
{
  name = "transcode";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2;
  nodes.box = {
    # the gate plays no part here: without it, a change to the gate
    # is not a reason to run this again
    dd.verify.enable = false;
    imports = [
      ./box.nix
      ../modules/library/transcode.nix
    ];
    dd.transcode.enable = true;
    # the stand-in for the bucket, and the only thing a session may fetch
    dd.transcode.source = "http://127.0.0.1:8000/";
    environment.systemPackages = [
      pkgs.ffmpeg-headless
      pkgs.curl
      pkgs.rclone
      (pkgs.python3.withPackages (p: [ p.pynacl ]))
    ];
  };
  scriptEnv = {
    chunker = ./transcode-chunks.py;
  };
}
