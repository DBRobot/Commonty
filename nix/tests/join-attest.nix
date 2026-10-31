# A box's join proving its TPM (docs/dev/attestation.md), on a machine
# booted the way a member's box boots: OVMF with Secure Boot under keys of
# the box's own, lanzaboote's signed image, a TPM (swtpm) whose EK
# certificate a test maker issued. Its join and answer go through the
# agreed state's own judgement (attest-check join: state.Block, the box
# voters' challenges, the TPM's answer). A good box joins; an EK not from a
# maker's CA, a PCR 4 not the release's, Secure Boot off are refused, with
# why; an update to a new boot image keeps the join key opening, with no
# one doing anything, and the box proves itself under the new release.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  sys = pkgs.stdenv.hostPlatform.system;
  inherit (self.packages.${sys}) attest-check;
  lanzaboote = self.inputs.lanzaboote;
  ca =
    name:
    pkgs.runCommand "attest-test-${name}" { nativeBuildInputs = [ pkgs.openssl ]; } ''
      mkdir $out
      openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=${name} \
        -days 3650 -keyout key.pem -out $out/ca.pem
      openssl pkcs8 -topk8 -nocrypt -in key.pem -out $out/ca.p8
    '';
  # the TPM's maker, and one that is not
  maker = ca "test-tpm-maker";
  other = ca "not-the-maker";
in
{
  name = "join-attest";
  node.specialArgs = { inherit self; };
  nodes.box =
    { lib, ... }:
    {
      imports = [
        "${lanzaboote}/nix/tests/lanzaboote/common/lanzaboote.nix"
        lanzaboote.nixosModules.lanzaboote
        ../modules/box/boot-key.nix
        # lanzaboote's test image is made for a newer nixpkgs, whose
        # repart image has a switch; ours makes it whenever imported
        { options.image.repart.enable = lib.mkEnableOption "the repart image"; }
      ];
      lanzabooteTest.persistentRoot = true;
      # room on its root for the AK's files and a few boots' state
      image.repart.partitions.root.repartConfig.SizeMinBytes = lib.mkForce "256M";
      virtualisation = {
        useSecureBoot = true;
        # with the TPM's measurements too: the event log, PCRs 4 and 7
        efi.OVMF = pkgs.OVMFFull.fd;
        memorySize = 1536;
        tpm.enable = true;
        # the EK certificate, in NV where a TPM maker puts it: once, the
        # TPM's state kept across its restarts
        tpm.provisioning = ''
          set -euo pipefail
          ${pkgs.tpm2-tools}/bin/tpm2_nvreadpublic 0x1C00002 >/dev/null 2>&1 && exit 0
          w=$(mktemp -d)
          ${pkgs.tpm2-tools}/bin/tpm2_createek -Q -c "$w/ek.ctx" -G rsa -u "$w/ek.pub"
          ${pkgs.tpm2-tools}/bin/tpm2_flushcontext -t
          ${attest-check}/bin/attest-check make-ek-cert -ek "$w/ek.pub" -ca-key ${maker}/ca.p8 -ca-cert ${maker}/ca.pem -o "$w/ek.der"
          ${pkgs.tpm2-tools}/bin/tpm2_nvdefine -Q 0x1C00002 -C o -s "$(stat -c %s "$w/ek.der")" -a 'ownerread|ownerwrite|authread|authwrite'
          ${pkgs.tpm2-tools}/bin/tpm2_nvwrite -Q 0x1C00002 -C o -i "$w/ek.der"
          rm -rf "$w"
        '';
      };
      # the next release's boot: another image (its command line differs),
      # on the ESP beside this one, as lzbt puts a new generation's
      specialisation.update.configuration.boot.kernelParams = [ "dd.test=update" ];
      environment.systemPackages = [
        attest-check
        pkgs.tpm2-tools
        pkgs.jq
        (import ../lib/tpm-policy.nix pkgs)
      ];
      environment.variables.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
      environment.etc = {
        # the stub the release lists, as flake.nix's attestBoot takes it
        "dd-test/stub.efi".source = "${lanzaboote.packages.${sys}.stub}/bin/lanzaboote_stub.efi";
        "dd-test/roots/00001014/ca.pem".source = "${maker}/ca.pem";
        "dd-test/other/00001014/ca.pem".source = "${other}/ca.pem";
        # the release's own roots: AMD, Intel, Infineon and the rest
        "dd-test/fleet-roots".source = ../../fleet/tpm-roots;
      };
      documentation.enable = false;
    };
  scriptEnv =
    { nodes }:
    {
      image = "${nodes.box.system.build.image}/${nodes.box.image.fileName}";
      # its system, and the update's beside it (specialisation/update)
      system = "${nodes.box.system.build.toplevel}";
      qemuImg = "${nodes.box.virtualisation.qemu.package}/bin/qemu-img";
    };
}
