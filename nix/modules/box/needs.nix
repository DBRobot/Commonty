# What every box runs to be one of the fleet (types: nix/lib/needs-type.nix)
{
  dd-control = {
    what = "the control plane: heartbeats, leases and the directory, agreed by the voting boxes and a witness";
    everyBox = true;
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
    units = [ "dd-build" ];
    everyBox = true;
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

  dd-box-report = {
    what = "what this box is (detect.sh), signed by its TPM: the report its config is built from";
    units = [
      "dd-locate"
      "dd-ports-check"
    ];
    everyBox = true;
    base = {
      requires.tpm = true;
      # node1, node2, 2026-10-04: peak
      uses = {
        memory = "20M";
        storage = "0";
      };
      network = {
        outbound = "none";
        inbound = "none";
      };
      depends = { };
      privacy = "none"; # hardware, never data
    };
  };

  dd-secrets-key = {
    what = "the box's secrets key, made in its TPM: what opens its own sops file and nothing else";
    everyBox = true;
    base = {
      requires.tpm = true;
      # node1, 2026-10-04: peak
      uses = {
        memory = "6M";
        storage = "0";
      };
      network = {
        outbound = "none";
        inbound = "none";
      };
      depends = { };
      privacy = "none";
    };
  };

  dd-mail = {
    what = "mail from the box to members and the owner, signed by its TPM and handed to the mail Worker (modules/mail.nix)";
    # one instance per message, behind the dd-mail socket
    units = [ "dd-mail@" ];
    base = {
      requires.tpm = true;
      uses = {
        memory = "8M";
        storage = "0";
        estimated = true; # a curl and the TPM tools, for each message
      };
      network = {
        outbound = [ "mail" ]; # mail.<domain>, the mail Worker
        inbound = "localhost"; # a socket for the box's own services
      };
      depends = { };
      # each message passes through as it was written, on its way to
      # Cloudflare (which can read it too); none is kept
      privacy = "plaintext";
    };
  };

  disk-unlock = {
    what = "the encrypted pools open at boot from the TPM, under the signed boot it measures; over the network from the owner's paper key when it cannot";
    units = [
      "dd-disk-check"
      "dd-disk-key-forget"
    ];
    everyBox = true;
    base = {
      requires = {
        tpm = true;
        uefi = true;
        wiredInitrd = true;
      };
      uses = {
        memory = "0";
        storage = "0";
      }; # the initrd's, gone by the time the box is up
      network = {
        outbound = "none";
        inbound = "lan"; # ssh in the initrd, for the paper key
      };
      depends = { };
      privacy = "none";
    };
  };

  zfs-datasets = {
    what = "the datasets the box's services ask for, made and set on its pools (never destroyed)";
    everyBox = true;
    # snapshots older than deleted data may last (storage/snapshots.nix)
    units = [
      "dd-snapshot-trim"
      "zfs-snapshot-*"
    ];
    base = {
      # node1, 2026-10-04: peak
      uses = {
        memory = "3M";
        storage = "0";
      };
      network = {
        outbound = "none";
        inbound = "none";
      };
      depends = { };
      privacy = "none";
    };
  };

  home-network = {
    what = "the box on the house network: by cable when it has one, by wifi when not, at the address the router reserves (box/home-network.nix)";
    units = [
      "NetworkManager-ensure-profiles"
      "dd-wifi-apply"
      "dd-wifi-status"
    ];
    everyBox = true;
    base = {
      uses = {
        memory = "8M";
        storage = "0";
        estimated = true;
      };
      network = {
        outbound = "none";
        inbound = "none";
      };
      privacy = "none";
    };
  };
}
