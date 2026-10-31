# What the libraries need of a box (types: nix/lib/needs-type.nix)
{
  libraries = {
    what = "members' files, films and shows over WebDAV, ciphertext made on their devices, kept in the storage pool";
    # ciphertext
    trust = "any";
    # ciphertext: members' libraries, read and written through the gate
    traffic = {
      class = "high";
      upMbps = 25;
    };
    alwaysOn = true;
    buckets.libraries = { };
    base = {
      # the files are the pool's; the gate moves them
      uses = {
        memory = "0";
        storage = "0";
      };
      network = {
        outbound = [ "store" ];
        inbound = "network";
      };
      depends = {
        dd-verify.local = true;
        nginx.local = true;
        store.local = true;
      };
      privacy = "none"; # names and bytes encrypted on the device
    };
  };

  disk-images = {
    what = "members' archives of old computers (`dd image`), restic's layout, encrypted on the device, kept in the storage pool";
    # ciphertext
    trust = "any";
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
        outbound = [ "store" ];
        inbound = "network";
      };
      depends = {
        dd-verify.local = true;
        nginx.local = true;
        store.local = true;
      };
      privacy = "none";
    };
  };

  mail-inbox = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "members' mail, locked to their key by the mail Worker before any box has it, kept in the storage pool; and the pipe their app sends mail through";
    # ciphertext, and who a member sends to
    trust = "any";
    # ciphertext: mail the Worker locked, and TLS the member's app runs
    traffic = {
      class = "low";
      upMbps = 2;
    };
    alwaysOn = true;
    buckets.mail = { };
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
        store.local = true;
        # its pipe is files.<domain>/_dd/mail/, a name of the gateway's
        # (`*`): on the box holding the network's lease
        headscale.local = true;
      };
      privacy = "metadata"; # locked to the member's key before it arrives; the pipe shows which server, and when
    };
  };

  dd-transcode = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
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
        outbound = [ "store" ]; # the library, by range, from the store on loopback
        inbound = "localhost"; # files.<domain>'s nginx in front
      };
      depends = {
        nginx.local = true;
        dd-verify.local = true;
        # it plays at files.<domain>/_dd/transcode/, a name of the
        # gateway's (`*`): on the box holding the network's lease
        headscale.local = true;
      };
      privacy = "plaintext"; # a film, in memory while it plays; never on disk
    };
  };
}
