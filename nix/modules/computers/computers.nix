{
  config,
  ddScript,
  pkgs,
  lib,
  self,
  ...
}:
let
  cfg = config.dd.computers;
  base = config.dd.domain;
  # each computer's directory, its own user's alone (DynamicUser=,
  # StateDirectory=dd-computer/<id>, so in /var/lib/private, which no other
  # user can enter)
  dir = "/var/lib/dd-computer";
  private = "/var/lib/private/dd-computer";
  # the network key each computer here was last handed, root's alone
  netKeys = "/run/dd-computer-net";
  # whose each computer here is, as the agreed state said when it was last
  # prepared: root's, outside every computer's directory, what the usage
  # count goes by
  owners = "/var/lib/dd-computer-owners";
  placed = "/run/dd-placed";
  sock = "/run/dd-computers/dd-computers.sock";
  policy = (lib.importJSON ../../../fleet/policy.json).computers;
  bin = "${self.packages.${pkgs.stdenv.hostPlatform.system}.computers}/bin/dd-computers";
  # the guest's way to the fleet's control server, through the front door
  # (as a box's: modules/net/box.nix)
  netbridge = self.packages.${pkgs.stdenv.hostPlatform.system}.netbridge;
  control = self.packages.${pkgs.stdenv.hostPlatform.system}.dd-control;
  # what the services this box is built with keep aside (the needs' sizes,
  # GiB), never given to a computer
  fp = import ../../lib/fleet-placement.nix { inherit lib; };
  brought = lib.attrByPath [ "dd" "built" ] [ ] config;
  keptGiB = lib.foldl' (a: s: a + (fp.capability.sizes.${s}.memory or 0)) 0 (
    lib.filter (s: s != "computers") brought
  );
  # the box itself, and before the services are measured, all of them
  baseMiB = 2048;
  budgetMiB = baseMiB + builtins.ceil (keptGiB * 1024);
  # the guest's own nixpkgs, without the box's overlays: the guest rebuilds
  # from plain nixpkgs (guest/base.nix), so the host's first system is what
  # the guest would build (guest/system.nix says how it differs)
  guestSystem = import ./guest/system.nix {
    inherit pkgs netbridge;
    member = ./guest/member.nix;
  };
  # the system disk every computer starts from: copied at its first start,
  # and again at a start whose release made another (the run script)
  image = reproducible madeImage;
  # Every box rebuilds and signs what it runs, so the image must be the
  # same bytes on each; make-disk-image's is not. Its tree is kept and the
  # file system made again from it by mkfs (sorted, fixed seed, times
  # clamped), with the VM's clock past the epoch so every time clamps, nix's
  # paths renumbered, perl's hash order and the MBR's disk id fixed
  epoch = "315532800";
  reproducible =
    img:
    let
      install = "NIXOS_INSTALL_BOOTLOADER=1 nixos-enter";
    in
    assert lib.assertMsg (lib.hasInfix install img.buildCommand)
      "make-disk-image no longer runs ${install}";
    lib.overrideDerivation img (old: {
      buildCommand =
        "date -s @${toString (lib.toInt epoch + 1)} >/dev/null\nexport PERL_HASH_SEED=0 PERL_PERTURB_KEYS=0\n"
        +
          lib.replaceStrings
            [ install ]
            [
              "touch -h -d @${epoch} $mountPoint/nix/var/nix/profiles/system-1-link\n${install}"
            ]
            old.buildCommand
        + ''
          mount /dev/vda1 /mnt
          db=/mnt/nix/var/nix/db
          ${pkgs.sqlite}/bin/sqlite3 -bail $db/db.sqlite < ${renumber}
          rm -f $db/db.sqlite-shm $db/db.sqlite-wal
          # every inode's atime fixed, and its ctime moved past the epoch;
          # mkfs keeps /var/empty's immutable flag
          chattr -i /mnt/var/empty
          find /mnt -xdev -exec touch -h -a -d @${epoch} {} +
          chattr +i /mnt/var/empty
          mount -o remount,ro /mnt
          truncate -s "$(blockdev --getsize64 /dev/vda1)" /tmp/xchg/root.ext4
          SOURCE_DATE_EPOCH=${epoch} mkfs.ext4 -q -F -b 4096 -L nixos -U ${uuid} \
            -E hash_seed=${uuid},lazy_itable_init=0,lazy_journal_init=0,nodiscard \
            -d /mnt /tmp/xchg/root.ext4
          umount /mnt
          e2fsck -fn /tmp/xchg/root.ext4
        '';
      postVM = ''
        eval "$(partx "$diskImage" -o START --nr 1 --pairs)"
        [ "$START" = 2048 ] || { echo "root partition at sector $START, not 2048" >&2; exit 1; }
        dd if=xchg/root.ext4 of="$diskImage" bs=1M seek=1 conv=notrunc status=none
        rm xchg/root.ext4
        ${pkgs.util-linux}/bin/sfdisk --disk-id "$diskImage" 0x64640001
      ''
      + old.postVM;
    });
  # nix-store --load-db numbers the paths in no fixed order: by path
  # instead, registered at the epoch. A table this does not know fails it
  renumber = pkgs.writeText "renumber.sql" ''
    CREATE TEMP TABLE t AS SELECT group_concat(name) AS names
      FROM (SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name);
    SELECT CASE WHEN names = 'DerivationOutputs,Refs,SchemaMigrations,ValidPaths,sqlite_sequence'
      THEN 1 ELSE abs(-9223372036854775807 - 1) END FROM t;
    CREATE TEMP TABLE m AS SELECT id AS old, row_number() OVER (ORDER BY path) AS new FROM ValidPaths;
    CREATE TEMP TABLE v AS SELECT m.new AS id, path, hash, ${epoch} AS registrationTime,
      deriver, narSize, ultimate, sigs, ca FROM ValidPaths JOIN m ON m.old = ValidPaths.id;
    CREATE TEMP TABLE r AS SELECT a.new AS referrer, b.new AS reference
      FROM Refs JOIN m a ON a.old = Refs.referrer JOIN m b ON b.old = Refs.reference;
    CREATE TEMP TABLE d AS SELECT m.new AS drv, DerivationOutputs.id AS id, path
      FROM DerivationOutputs JOIN m ON m.old = DerivationOutputs.drv;
    DELETE FROM Refs;
    DELETE FROM DerivationOutputs;
    DELETE FROM ValidPaths;
    INSERT INTO ValidPaths SELECT * FROM v ORDER BY id;
    INSERT INTO Refs SELECT * FROM r ORDER BY referrer, reference;
    INSERT INTO DerivationOutputs SELECT * FROM d ORDER BY drv, id;
    PRAGMA journal_mode = DELETE;
    VACUUM;
  '';
  # make-disk-image's own default, which the guest's grub.cfg searches by
  uuid = "F222513B-DED1-49FA-B591-20CE86A2FE7F";
  madeImage = import "${pkgs.path}/nixos/lib/make-disk-image.nix" {
    inherit pkgs lib;
    rootFSUID = uuid;
    inherit (guestSystem) config;
    name = "dd-computer-image";
    # compressed: copied once per computer, and through the build cache
    format = "qcow2-compressed";
    partitionTableType = "legacy";
    diskSize = "auto";
    additionalSpace = "2G";
    copyChannel = false;
    configFile = pkgs.writeText "configuration.nix" ''
      {
        imports = [ ${./guest/base.nix} ./member.nix ];
        dd.guest.base = builtins.storePath ${./guest/base.nix};
        dd.guest.nixpkgs = builtins.storePath ${pkgs.path};
        dd.guest.netbridge = builtins.storePath ${netbridge};
      }
    '';
    contents = [
      {
        source = ./guest/member.nix;
        target = "/etc/nixos/member.nix";
      }
      # the empty Nix file, as the box writes it for a computer whose owner
      # wrote none, already applied: the system on this disk is the one it
      # would build, and a first boot does not rebuild it (a rebuild's
      # evaluation takes most of a small computer's memory)
      {
        source = pkgs.writeText "tried" (
          builtins.hashString "sha256" (builtins.readFile ./guest/member.nix)
        );
        target = "/var/lib/dd/tried";
      }
    ];
  };
  range = "${toString cfg.portBase}:${toString (cfg.portBase + cfg.portCount - 1)}";
  env = {
    DD_COMPUTERS_DIR = dir;
    DD_COMPUTERS_PLACED = placed;
    DD_COMPUTERS_CONTROL = "127.0.0.1:4195";
    DD_COMPUTERS_MEMORY_MIB = toString cfg.memoryMiB;
    DD_COMPUTERS_THREADS = toString cfg.threads;
    DD_COMPUTERS_DISK_GIB = toString cfg.diskGiB;
    DD_COMPUTERS_PORT_BASE = toString cfg.portBase;
    DD_COMPUTERS_PORT_COUNT = toString cfg.portCount;
  };
  # what every unit of the box's side keeps to
  hardened = {
    NoNewPrivileges = true;
    ProtectSystem = "strict";
    ProtectHome = true;
    PrivateTmp = true;
  };
  # A computer's own units (prepare, the guest, its share) run as a user of
  # its own, dd-computer-<id>, made for it by systemd, the only one that
  # owns its directory (0700): a guest that broke out into its QEMU holds
  # its own disks and nothing of another's
  perComputer = hardened // {
    DynamicUser = true;
    User = "dd-computer-%i";
    StateDirectory = "dd-computer/%i";
    StateDirectoryMode = "0700";
  };

  # One computer, by id, from the record dd-computer-prepare@ wrote from
  # the agreed state: its size and its ports. The disks are made the first
  # time and grown when the record grows; never shrunk.
  run = pkgs.writeShellScript "dd-computer-run" ''
    set -eu
    id="$1"
    case "$id" in *[!0-9a-f]* | "") echo "not an id: $id" >&2; exit 1 ;; esac
    d="$STATE_DIRECTORY"
    rec="$d"/record.json
    qmp="$RUNTIME_DIRECTORY"/qmp
    serial="$RUNTIME_DIRECTORY"/serial
    # prepared for this start (dd-computer-prepare@), or not at all: it
    # runs again on its own after a while, and prepare with it
    rm "$d"/.prepared 2>/dev/null || { echo "computer-$id was not prepared for this start" >&2; exit 1; }
    [ -s "$rec" ] || { echo "no record for $id" >&2; exit 1; }
    jq() { ${pkgs.jq}/bin/jq "$@"; }
    img() { ${pkgs.qemu-utils}/bin/qemu-img "$@"; }
    int() { jq -er "$1 | select(type == \"number\" and . == floor and . > 0)" "$rec"; }
    mem=$(int .memory)
    threads=$(int .threads)
    disk=$(int .disk)
    fwd=$(jq -er '[{guest: 22, host: .port}] + .forwards
      | map(select((.host | type) == "number" and (.guest | type) == "number"))
      | map("hostfwd=tcp::\(.host)-:\(.guest)") | join(",")' "$rec")
    # what the guest says of its last apply: a raw disk of fixed size, not
    # a share it could fill; nothing on the box reads it
    [ -e "$d"/report.img ] || ${pkgs.coreutils}/bin/truncate -s 1M "$d"/report.img
    # the system disk is the release's: made again from its image when the
    # image is not the one this disk was made from (a computer from before
    # this was kept has no record of it, and is made again too). /home is
    # its own disk and stays; the guest's host keys are on it
    if [ ! -e "$d"/system.qcow2 ] || [ "$(cat "$d"/system.base 2>/dev/null)" != ${image} ]; then
      rm -f "$d"/system.base
      img convert -O qcow2 ${image}/nixos.qcow2 "$d"/system.qcow2.new
      mv "$d"/system.qcow2.new "$d"/system.qcow2
      echo ${image} > "$d"/system.base
    fi
    [ -e "$d"/home.qcow2 ] || img create -q -f qcow2 "$d"/home.qcow2 "$disk"G
    for f in system home; do
      have=$(img info --output=json "$d/$f.qcow2" | jq '.["virtual-size"]')
      if [ "$have" -lt $(( disk * 1024 * 1024 * 1024 )) ]; then
        img resize -q "$d/$f.qcow2" "$disk"G
      fi
    done
    ${lib.optionalString cfg.network ''
      # the network key prepare was handed for this start, or none
      rm -f "$d"/config/net.json
      if jq -e '(.key | type) == "string"' "$CREDENTIALS_DIRECTORY"/net.json >/dev/null 2>&1; then
        (umask 077 && cp "$CREDENTIALS_DIRECTORY"/net.json "$d"/config/net.json.new \
          && mv "$d"/config/net.json.new "$d"/config/net.json)
      fi
    ''}
    rm -f "$qmp" "$serial"
    if [ -w /dev/kvm ]; then accel="accel=kvm -cpu host"; else accel="accel=tcg -cpu max"; fi
    exec ${pkgs.qemu_kvm}/bin/qemu-system-x86_64 \
      -name computer-"$id" -machine q35,$accel -m "$mem" -smp "$threads" \
      -nodefaults -display none -vga none \
      -drive file="$d"/system.qcow2,if=none,id=system,format=qcow2 \
      -device virtio-blk-pci,drive=system,serial=system,bootindex=0 \
      -drive file="$d"/home.qcow2,if=none,id=home,format=qcow2 \
      -device virtio-blk-pci,drive=home,serial=home \
      -virtfs local,path="$d"/config,mount_tag=config,security_model=none,readonly=on \
      -drive file="$d"/report.img,if=none,id=report,format=raw \
      -device virtio-blk-pci,drive=report,serial=report \
      -netdev user,id=n0,"$fwd" -device virtio-net-pci,netdev=n0 \
      -device virtio-rng-pci \
      -qmp unix:"$qmp",server=on,wait=off \
      -chardev socket,id=console,path="$serial",server=on,wait=off,logfile="$d"/console.log,logappend=off \
      -serial chardev:console
  '';
  # A network key for a computer kept to its owner's own boxes, asked of
  # the gate by this box, which holds its lease, signed by its TPM; the
  # answer opens only with the box's secrets key. Root's alone, outside the
  # computer's directory: dd-computer@ is handed it as a credential and
  # puts it in the guest's share as its own user (its directory is
  # systemd's idmapped one, nobody's on disk, so root never writes in it).
  # Single use, ten minutes; none, and the guest is simply not on the
  # Commonty network this time.
  # on a joined box the door goes by whose box the plan picked, whichever
  ownOnly =
    if config.dd.box.joined then
      "true"
    else
      lib.removeSuffix "\n" ''
        [ "$(${pkgs.curl}/bin/curl -fsS --max-time 10 http://127.0.0.1:4195/computers \
          | ${pkgs.jq}/bin/jq -r --arg id "$id" '.computers[$id].consent // empty')" = own ] || exit 0
      '';
  netJoin = pkgs.writeShellScript "dd-computer-net" ''
    set -u
    id="$1"
    case "$id" in *[!0-9a-f]* | "") exit 1 ;; esac
    ${pkgs.coreutils}/bin/install -d -m 0700 ${netKeys}
    out=${netKeys}/"$id".json
    # always one, so dd-computer@'s credential is there: empty, no key
    (umask 077 && echo '{}' > "$out.new" && ${pkgs.coreutils}/bin/mv "$out.new" "$out")
    # where it may run, as agreed: never what its own directory says
    ${ownOnly}
    export PATH=${
      lib.makeBinPath [
        pkgs.age
        pkgs.age-plugin-tpm
        pkgs.tpm2-tools
        pkgs.curl
        pkgs.jq
        pkgs.coreutils
      ]
    }
    tmp=$(mktemp)
    trap 'rm -f "$tmp"' EXIT
    # the door or the TPM can be busy a moment (a release switching): a few
    # tries before the computer starts without one, a minute apart, as the
    # door answers one a minute for a computer
    for try in 1 2 3; do
      if ${control}/bin/dd-control -box ${config.networking.hostName} -sign-computer-join "$id" \
        | curl -fsS --max-time 30 -H 'content-type: application/json' --data-binary @- ${cfg.networkDoor} \
        | age -d -i /var/lib/dd/secrets-key.txt > "$tmp" \
        && jq -e '(.key | type) == "string" and (.control_url | startswith("https://")) and (.hostname | test("^computer-[0-9a-f]{16}$"))' "$tmp" >/dev/null \
        && (umask 077 && cp "$tmp" "$out.new" && mv "$out.new" "$out"); then
        echo "computer-$id: a key to the Commonty network handed in"
        exit 0
      fi
      [ "$try" = 3 ] || sleep 65
    done
    echo "computer-$id: no key to the Commonty network this time"
  '';
  # Whose it is, from the agreed state, into a file of root's outside its
  # directory: the usage count is by this, never by what the computer's
  # own user could have written
  noteOwner = pkgs.writeShellScript "dd-computer-owner" ''
    set -eu
    id="$1"
    case "$id" in *[!0-9a-f]* | "") exit 1 ;; esac
    owner=$(${pkgs.curl}/bin/curl -fsS --max-time 10 http://127.0.0.1:4195/computers \
      | ${pkgs.jq}/bin/jq -r --arg id "$id" '.computers[$id].owner // empty')
    [[ $owner =~ ^[a-z0-9][a-z0-9_-]{0,31}$ ]] || { echo "computer-$id: no owner agreed" >&2; exit 1; }
    install -d -m 0755 ${owners}
    printf '%s\n' "$owner" > ${owners}/"$id".new
    mv ${owners}/"$id".new ${owners}/"$id"
  '';
  # as a game's: the guest powers down, then qemu is told to quit
  stop = pkgs.writeShellScript "dd-computer-stop" ''
    qmp="$RUNTIME_DIRECTORY"/qmp
    [ -S "$qmp" ] || exit 0
    printf '%s\n' '{"execute":"qmp_capabilities"}' '{"execute":"system_powerdown"}' \
      | ${pkgs.socat}/bin/socat - UNIX-CONNECT:"$qmp" >/dev/null 2>&1 || true
    for i in $(seq 1 60); do kill -0 "$MAINPID" 2>/dev/null || exit 0; sleep 1; done
    printf '%s\n' '{"execute":"qmp_capabilities"}' '{"execute":"quit"}' \
      | ${pkgs.socat}/bin/socat - UNIX-CONNECT:"$qmp" >/dev/null 2>&1 || true
    for i in $(seq 1 15); do kill -0 "$MAINPID" 2>/dev/null || exit 0; sleep 1; done
  '';
in
{
  imports = [ ../guests/egress.nix ];

  # Members' remote computers (docs/dev/remote-computers.md), the box's
  # side. Each is agreed state; the plan puts it on a box and dd-control,
  # holding its lease (svc/computer-<id>), raises /run/dd-placed/computer-<id>
  # and starts dd-computer@<id> here, and stops it before the lease may
  # lapse. A deploy never restarts one.
  options.dd.computers = {
    enable = lib.mkEnableOption "members' remote computers on this box";
    portBase = lib.mkOption {
      type = lib.types.port;
      default = policy.portBase;
      readOnly = true;
      description = "the first port of the pool computers' SSH and forwards come from (fleet/policy.json computers; games have 27000 up)";
    };
    portCount = lib.mkOption {
      type = lib.types.ints.positive;
      default = policy.portCount;
      readOnly = true;
    };
    memoryMiB = lib.mkOption {
      type = lib.types.ints.positive;
      default = policy.memory;
      description = "what one member's computers here may take together (fleet/policy.json computers, as the judge holds them to)";
    };
    threads = lib.mkOption {
      type = lib.types.ints.positive;
      default = policy.threads;
    };
    diskGiB = lib.mkOption {
      type = lib.types.ints.positive;
      default = policy.disk;
    };
    reserveMiB = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.unsigned;
      default = null;
      description = "a fixed reserve the box keeps from computers, in place of its services' measured memory (null: measured, the needs' sizes until measured)";
    };
    headroom = lib.mkOption {
      type = lib.types.numbers.positive;
      default = 1.5;
      description = "what the services' measured peak is multiplied by before the box offers the rest to computers";
    };
    network = lib.mkOption {
      type = lib.types.bool;
      default = lib.attrByPath [ "dd" "net" "enable" ] false config;
      description = "hand a computer kept to its owner's own boxes a key to the Commonty network as it starts (tagged a computer)";
    };
    networkDoor = lib.mkOption {
      type = lib.types.str;
      default = "https://home.${base}/_dd/network/computer";
      description = "where this box asks for a computer's network key (the gate's network.rs)";
    };
    manager = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "the page and its API (compute.<domain>), and the firewall's filler";
    };
    reach = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      readOnly = true;
      default = [ "${toString cfg.portBase}-${toString (cfg.portBase + cfg.portCount - 1)}" ];
      description = "the ports a member's device may reach on this box, as headscale's policy writes them";
    };
    image = lib.mkOption {
      type = lib.types.package;
      readOnly = true;
      default = image;
      description = "the disk a new computer's system starts as";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.isx86_64;
        message = "members' computers are x86_64 guests for now";
      }
    ];
    dd.guests.enable = true;
    dd.box.plaintext = [ "members' computers (their disks and memory)" ];

    # the page's server: no computer's files are its
    users.users.dd-computers = {
      uid = 952;
      isSystemUser = true;
      group = "dd-computers";
    };
    users.groups.dd-computers.gid = 952;

    # The agreed record, read on loopback, which the guest's own unit can't
    # (its sockets carry dd-guests): checked here again and written where
    # dd-computer@ reads it. Fails unless this box holds the computer's flag
    # and the record is agreed up.
    systemd.services."dd-computer-prepare@" = {
      description = "The agreed record of member's computer %i";
      unitConfig.ConditionPathExists = "${placed}/computer-%i";
      environment = env;
      serviceConfig = perComputer // {
        Type = "oneshot";
        ExecStart = "${bin} prepare %i";
        # as root, which the TPM's key needs; whatever comes of it, the
        # computer starts
        ExecStartPost = [ "+${noteOwner} %i" ] ++ lib.optional cfg.network "-+${netJoin} %i";
        PrivateDevices = true;
        # the device policy is the cgroup's, so it holds for the "+" steps
        # too: netJoin signs with the TPM. root's alone by its mode; the
        # computer's own user never sees it in its private /dev
        DeviceAllow = lib.optional cfg.network "/dev/tpmrm0 rw";
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
      };
    };

    systemd.services."dd-computer@" = {
      description = "Member's computer %i";
      after = [
        "firewall.service"
        "dd-computer-prepare@%i.service"
      ];
      # wanted, not required: a prepare that failed (dd-control not up yet)
      # fails this start, and the restart asks again
      wants = [
        "firewall.service"
        "dd-computer-prepare@%i.service"
      ];
      # only where dd-control holds its lease (box/placed.nix's condition,
      # by instance)
      unitConfig.ConditionPathExists = "${placed}/computer-%i";
      restartIfChanged = false;
      stopIfChanged = false;
      unitConfig.X-StopOnRemoval = false;
      serviceConfig = perComputer // {
        # what it may reach out to (nix/modules/guests/egress.nix), and KVM
        SupplementaryGroups = [
          config.dd.guests.group
          "kvm"
        ];
        # QMP and the console, its own too
        RuntimeDirectory = "dd-computer/%i";
        RuntimeDirectoryMode = "0700";
        ExecStart = "${run} %i";
        ExecStop = "${stop} %i";
        LoadCredential = lib.optional cfg.network "net.json:${netKeys}/%i.json";
        TimeoutStopSec = 90;
        Restart = "on-failure";
        RestartSec = 20;
        DeviceAllow = [ "/dev/kvm rw" ];
      };
    };

    # a Nix file or a key saved reaches a running guest's share: each
    # computer's as its own user, asked for each that runs here
    systemd.services."dd-computer-share@" = {
      description = "Member's computer %i's share, as agreed now";
      unitConfig.ConditionPathExists = "${placed}/computer-%i";
      environment = env;
      restartIfChanged = false;
      serviceConfig = perComputer // {
        Type = "oneshot";
        ExecStart = "${bin} share %i";
        PrivateDevices = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
        ];
      };
    };
    systemd.services.dd-computers-share = {
      description = "Members' running computers' shares, as agreed now";
      restartIfChanged = false;
      path = [ pkgs.systemd ];
      serviceConfig.Type = "oneshot";
      script = ''
        for f in ${placed}/computer-*; do
          id=''${f##*/computer-}
          case "$id" in "" | *[!0-9a-f]*) continue ;; esac
          [ ''${#id} -eq 16 ] || continue
          if systemctl is-active -q dd-computer@"$id".service; then
            systemctl start --no-block dd-computer-share@"$id".service
          fi
        done
      '';
    };
    systemd.timers.dd-computers-share = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "30s";
        OnUnitActiveSec = "30s";
      };
    };

    # The page and its API: what the agreed state says of a member's
    # computers. It believes x-dd-user, so it listens on a socket only
    # nginx's group opens; it starts nothing
    systemd.services.dd-computers = lib.mkIf cfg.manager {
      description = "Members' computers: the page behind compute.${base}";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      environment = env // {
        DD_COMPUTERS_BIND = sock;
        DD_COMPUTERS_ACCESS = "/run/dd-computers/access.json";
        DD_COMPUTERS_BOXES = pkgs.writeText "boxes.json" (
          builtins.toJSON (
            lib.mapAttrs (_: b: b.net) (
              lib.filterAttrs (_: b: b ? net) (lib.importJSON ../../../fleet/boxes.json)
            )
          )
        );
      };
      serviceConfig = hardened // {
        RuntimeDirectory = "dd-computers";
        RuntimeDirectoryMode = "0750";
        ExecStart = "${bin} serve";
        Restart = "on-failure";
        User = "dd-computers";
        Group = "dd-computers";
        PrivateDevices = true;
      };
    };
    users.users.nginx = lib.mkIf (cfg.manager && config.services.nginx.enable) {
      extraGroups = [ "dd-computers" ];
    };
    # the gate stands in front of compute.<domain>: its /_dd/verify that
    # auth_request asks, the sign-in redirect, and a token's audience. Without
    # it nginx's subrequest found no location, a 404, and answered 500
    dd.verify.hosts = lib.mkIf cfg.manager [ "compute" ];
    services.nginx.virtualHosts."compute.${base}" = lib.mkIf cfg.manager {
      useACMEHost = base;
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://unix:${sock}:";
        extraConfig = ''
          auth_request /_dd/verify;
          auth_request_set $auth_user $upstream_http_x_auth_request_preferred_username;
          proxy_set_header X-DD-User $auth_user;
          auth_request_set $auth_role $upstream_http_x_dd_role;
          proxy_set_header X-DD-Role $auth_role;
          proxy_set_header Authorization "";
          client_max_body_size 16k;
          error_page 401 = @login;
          error_page 403 = @waiting;
        '';
      };
    };

    # Who may reach each computer running here: its owner's devices and the
    # friends its record names, asked of the gate every fifteen seconds, as
    # for games (games-access.sh, the same script on its own chain)
    systemd.services.computers-access = lib.mkIf cfg.manager {
      description = "Open each member's computer to its owner's and invited devices";
      restartIfChanged = false;
      after = [ "firewall.service" ];
      path = [
        pkgs.jq
        pkgs.curl
        pkgs.iptables
        pkgs.coreutils
        pkgs.diffutils
      ];
      serviceConfig.Type = "oneshot";
      script = ddScript ../games/games-access.sh {
        ACCESS = "/run/dd-computers/access.json";
        GATE = "http://127.0.0.1:${toString config.dd.verify.port}";
        STATE = "/run/computers-access";
        CHAIN = "dd-computers";
      };
    };
    systemd.timers.computers-access = lib.mkIf cfg.manager {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "30s";
        OnUnitActiveSec = "15s";
        AccuracySec = "1s";
      };
    };
    systemd.services.firewall.postStart = lib.mkIf cfg.manager (
      lib.mkAfter "systemctl start --no-block computers-access.service || true"
    );

    # what each member's computers keep here, into the facts for their allowance
    systemd.services.dd-computers-usage = {
      description = "Count what each member's computers keep";
      path = [
        pkgs.jq
        pkgs.coreutils
        pkgs.findutils
      ];
      restartIfChanged = false;
      serviceConfig.Type = "oneshot";
      script = ddScript ../guests/usage.sh {
        DIR = private;
        KIND = "computers";
        OWNERS = owners;
      };
    };
    systemd.timers.dd-computers-usage = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "5min";
        OnUnitActiveSec = "1h";
      };
    };

    # how big a computer this box can take, and what each here holds: into
    # the heartbeat, for placement (computer-facts.sh)
    systemd.services.dd-computer-facts = {
      description = "Say how big a computer this box can take";
      path = [
        pkgs.coreutils
        pkgs.gawk
        pkgs.gnugrep
        pkgs.findutils
      ];
      restartIfChanged = false;
      serviceConfig.Type = "oneshot";
      script = ddScript ./computer-facts.sh {
        DIR = private;
        RESERVE_BYTES = (if cfg.reserveMiB != null then cfg.reserveMiB else budgetMiB) * 1024 * 1024;
        MEASURED = lib.optionalString (cfg.reserveMiB == null) "/var/lib/dd-facts/services.prom";
        BASE_BYTES = baseMiB * 1024 * 1024;
        HEADROOM = cfg.headroom;
        # six missed 5-minute measures
        STALE_SECONDS = 1800;
      };
    };
    systemd.timers.dd-computer-facts = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "1min";
        OnUnitActiveSec = "1min";
      };
    };

    # SSH and forwards are open on the Commonty network only, to whom the
    # owner lets in (computers-access fills the chain)
    networking.firewall.extraCommands = ''
      for ipt in iptables ip6tables; do
        $ipt -N dd-computers 2>/dev/null || true
        $ipt -D nixos-fw -i commonty0 -p tcp --dport ${range} -j dd-computers 2>/dev/null || true
        $ipt -I nixos-fw 1 -i commonty0 -p tcp --dport ${range} -j dd-computers
      done
    '';
  };
}
