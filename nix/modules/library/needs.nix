# What the libraries need of a box (types: nix/lib/needs-type.nix)
{
  libraries = {
    what = "members' files, films and shows over WebDAV, ciphertext made on their devices, kept in the storage pool";
    # ciphertext: members' libraries, read and written through the gate
    traffic = {
      class = "high";
      upMbps = 25;
    };
    alwaysOn = true;
    base = {
      # the files are the pool's; the gate moves them
      uses = {
        memory = "0";
        storage = "0";
      };
      network = {
        outbound = [ "s3" ];
        inbound = "network";
      };
      depends = {
        dd-verify.local = true;
        nginx.local = true;
        garage.local = false;
      };
      privacy = "none"; # names and bytes encrypted on the device
    };
  };

  disk-images = {
    what = "members' archives of old computers (`dd image`), restic's layout, encrypted on the device, kept in the storage pool";
    # ciphertext: members' disk images
    traffic = {
      class = "high";
      upMbps = 20;
    };
    base = {
      uses = {
        memory = "0";
        storage = "0";
      }; # the gate's and the pool's
      network = {
        outbound = [ "s3" ];
        inbound = "network";
      };
      depends = {
        dd-verify.local = true;
        nginx.local = true;
        garage.local = false;
      };
      privacy = "none";
    };
  };

  mail-inbox = {
    what = "members' mail, locked to their key by the mail Worker before any box has it, kept in the storage pool; and the pipe their app sends mail through";
    # ciphertext: mail the Worker locked, and TLS the member's app runs
    traffic = {
      class = "low";
      upMbps = 2;
    };
    alwaysOn = true;
    units = [ "dd-mail-hold-sign*" ];
    base = {
      uses = {
        memory = "0";
        storage = "0";
      };
      network = {
        # port 25 of whichever server a member's mail goes to, and s3 and
        # mail.<domain>
        outbound = "internet";
        inbound = "network";
      };
      depends = {
        dd-verify.local = true;
        garage.local = false;
      };
      privacy = "none"; # locked to the member's key before it arrives
    };
  };

  dd-transcode = {
    what = "plays a film a member's device cannot, decrypting it in memory only";
    # it decrypts the film it transcodes, for any member
    trust = "owner";
    traffic = {
      class = "high";
      upMbps = 25;
    };
    base = {
      # four sessions of 256M decrypted pieces each, and each one's
      # segments: twelve minutes, at most 384M (main.rs: BEHIND, AHEAD,
      # WINDOW_BYTES), and their ffmpegs
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
