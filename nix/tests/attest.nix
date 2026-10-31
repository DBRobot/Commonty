# Attestation end to end against a TPM (docs/dev/attestation.md): swtpm,
# its EK certified at start by a test CA under swtpm's vendor id, booted
# through OVMF with TPM support so the firmware's event log is real. The
# box's side and the validators' run as one program (attest-check), with
# the same code dd-control runs: Begin, two voters' challenges, the answer,
# the judgement. Refused without the software-TPM switch; refused without
# the no-Secure-Boot switch, OVMF having no keys enrolled.
{ pkgs, self, ... }:
let
  inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) attest-check;
  ca = pkgs.runCommand "attest-test-ca" { nativeBuildInputs = [ pkgs.openssl ]; } ''
    mkdir $out
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=test-tpm-maker \
      -days 3650 -keyout key.pem -out $out/ca.pem
    openssl pkcs8 -topk8 -nocrypt -in key.pem -out $out/ca.p8
  '';
in
{
  name = "attest";
  nodes.box = {
    virtualisation = {
      tpm.enable = true;
      # the EK certificate, in NV where a TPM maker puts it
      tpm.provisioning = ''
        set -euo pipefail
        w=$(mktemp -d)
        ${pkgs.tpm2-tools}/bin/tpm2_createek -Q -c "$w/ek.ctx" -G rsa -u "$w/ek.pub"
        ${pkgs.tpm2-tools}/bin/tpm2_flushcontext -t
        ${attest-check}/bin/attest-check make-ek-cert -ek "$w/ek.pub" -ca-key ${ca}/ca.p8 -ca-cert ${ca}/ca.pem -o "$w/ek.der"
        ${pkgs.tpm2-tools}/bin/tpm2_nvdefine -Q 0x1C00002 -C o -s "$(stat -c %s "$w/ek.der")" -a 'ownerread|ownerwrite|authread|authwrite'
        ${pkgs.tpm2-tools}/bin/tpm2_nvwrite -Q 0x1C00002 -C o -i "$w/ek.der"
        rm -rf "$w"
      '';
      useBootLoader = true;
      useEFIBoot = true;
      efi.OVMF = pkgs.OVMFFull.fd;
      memorySize = 1024;
    };
    boot.loader.systemd-boot.enable = true;
    boot.loader.efi.canTouchEfiVariables = true;
    environment.systemPackages = [
      pkgs.tpm2-tools
      attest-check
    ];
    environment.variables.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
    environment.etc."attest-roots/00001014/ca.pem".source = "${ca}/ca.pem";
    documentation.enable = false;
  };
}
