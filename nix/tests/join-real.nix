# The whole imaging path, with nothing standing in: the drive image.nix makes
# boots under OVMF and swtpm as a USB stick, and its screen installs the real
# lib.joinedBox, evaluated from a flake of this repository served over git and
# built by the box itself, onto a disk of the hardware minimum. It then starts
# what it installed: lanzaboote signed, Secure Boot on, the pool opened by the
# TPM, its join ticket read. join-install.nix is the same path's refusals, on
# a stand-in; this is the one that proves the build (docs/dev/joining.md).
#
# It is a real box's first install and takes the time one does, with the
# internet: nothing of the box's is in any store the VM reads. So it runs by hand
# outside the build sandbox, and is not one of CI's:
#   nix build .#checks.x86_64-linux.join-real.driver
#   JOIN_REAL_MINUTES=720 out=$(mktemp -d) result/bin/nixos-test-driver
# With JOIN_REAL_CACHE=1 the VM also substitutes from this machine's store: only
# what its own report changes is built, minutes instead of the hours.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  sys = pkgs.stdenv.hostPlatform.system;
  # the first boot's shell for the test; nobody gets one on a real box
  shell =
    { modulesPath, ... }:
    {
      imports = [ "${modulesPath}/testing/test-instrumentation.nix" ];
    };

  # where the VM finds the host (QEMU's user network)
  server = "http://10.0.2.2:8080";
  ticketId = "00112233445566778899aabbccddeeff";

  # This repository as the fleet's repository; a release signed by a key of
  # this build's alone, never the keyring on the machine running the test; and
  # a member's ticket. All made in this build's sandbox and only read after.
  fleet =
    pkgs.runCommand "join-real-fleet"
      {
        nativeBuildInputs = [
          pkgs.git
          pkgs.jq
          pkgs.biscuit-cli
        ];
        source = self.outPath;
        wrapper = pkgs.writeText "flake.nix" ''
          {
            inputs.repo.url = "git+${server}/repo.git?rev=@rev@";
            outputs = { self, repo }: {
              lib.joinedBox = a: (repo.lib.joinedBox a).extendModules { modules = [ ./shell.nix ]; };
            };
          }
        '';
        shellModule = pkgs.writeText "shell.nix" ''
          { modulesPath, ... }: { imports = [ "''${modulesPath}/testing/test-instrumentation.nix" ]; }
        '';
      }
      ''
        export HOME=$PWD DD_KEYRING_FILE=$PWD/keys.json
        export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
        export GIT_AUTHOR_DATE=1000000000 GIT_COMMITTER_DATE=1000000000
        repo() {
          git -C "$1" init -q
          git -C "$1" add -A -f
          git -C "$1" commit -qm "$2"
          git clone -q --bare "$1" "$out/$2.git"
          git -C "$out/$2.git" update-server-info
        }
        mkdir -p "$out/release" src flake
        cp -r --no-preserve=mode "$source/." src/
        repo src repo
        sed "s/@rev@/$(git -C "$out/repo.git" rev-parse HEAD)/" "$wrapper" > flake/flake.nix
        cp "$shellModule" flake/shell.nix
        repo flake box
        rev=$(git -C "$out/box.git" rev-parse HEAD)

        git init -q keys && mkdir keys/fleet
        ${self.packages.${sys}.dd}/bin/dd release init --repo keys | tail -1 > "$out/release.pub"
        jq -n --arg rev "$rev" '{counter: 1, rev: $rev, issued: 1000, boxes: {}}' > payload.json
        ${self.packages.${sys}.dd}/bin/dd release sign payload.json --out "$out/release/current.json"

        biscuit keypair --only-private-key > tk
        printf '%s\n' 'user("ada");' 'device("000000000000");' 'ticket("${ticketId}");' 'purpose("join");' \
          'check if time($t), $t < 4102444800;' 'check if operation("join");' \
          | biscuit generate --private-key-file tk - | tr -d '\n' > "$out/ticket"
      '';

  # the drive: installers.box's modules, with the test's shell and its fleet
  drive =
    (self.inputs.nixpkgs.lib.nixosSystem {
      system = sys;
      specialArgs = { inherit self; };
      modules = [
        ../installer/image.nix
        shell
        {
          # JOIN_REAL_CACHE (join-real.py): this machine's own store, to prove
          # the path without the hours. Nothing answers there otherwise
          nix.settings.extra-substituters = [ "http://10.0.2.2:8081" ];
          nix.settings.require-sigs = false;
          environment.etc."dd/test-release.pub".source = "${fleet}/release.pub";
          environment.variables = lib.mkForce {
            RELEASE_URL = "${server}/release/current.json";
            RELEASE_KEY = "/etc/dd/test-release.pub";
            SOURCE = "git+${server}/box.git";
            DETECT = "${../modules/box/report}/detect.sh";
            NOT_BEFORE = "100";
            FILES = "${server}/files";
          };
        }
      ];
    }).config.system.build.image;

  ovmf = pkgs.OVMF.override {
    secureBoot = true;
    msVarsTemplate = true;
  };
in
{
  name = "join-real";
  nodes = { };
  # a real box's install is hours; the test's own bound is JOIN_REAL_MINUTES
  globalTimeout = 16 * 3600;
  # 8 GB and 4 threads: what the hardware minimum lets through
  scriptEnv = {
    inherit fleet ticketId;
    drive = "${drive}";
    code = "${ovmf.firmware}";
    vars = "${ovmf.variables}";
    # a PC as it comes: Microsoft's keys in place
    factoryVars = "${ovmf.fd}/FV/OVMF_VARS.ms.fd";
    qemu = "${pkgs.qemu_kvm}/bin/qemu-system-x86_64";
    qemuImg = "${pkgs.qemu_kvm}/bin/qemu-img";
    swtpm = "${pkgs.swtpm}/bin/swtpm";
    zstd = "${pkgs.zstd}/bin/zstd";
    python = "${pkgs.python3}/bin/python3";
    nixServe = "${pkgs.nix-serve}/bin/nix-serve";
    git = "${pkgs.git}/bin/git";
  };
}
