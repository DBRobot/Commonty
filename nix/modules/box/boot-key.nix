# The join key across updates (docs/dev/attestation.md): a box whose key is
# under its approver (dd-boot-key enrol) approves, after each switch, what
# its ESP can now start, from the boot it is in; and at each boot, once the
# boot came up approved, moves its epoch on so only the last few good boots
# stay approved. A key made before (PCR 7 alone) is left as it is.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  pkg = self.packages.${pkgs.stdenv.hostPlatform.system};
  tools = [
    pkgs.tpm2-tools
    pkgs.coreutils
  ];
in
lib.mkIf config.boot.lanzaboote.enable {
  environment.systemPackages = [ pkg.dd-boot-key ];
  # a joined box's first boot makes its join key with it (disk-unlock-seal.sh)
  systemd.services.dd-unlock-seal = lib.mkIf (config.dd.unlock.tpmOnly or false) {
    path = [ pkg.dd-boot-key ];
  };
  # after a switch's lzbt install: the new generation's image is on the ESP
  system.activationScripts.dd-boot-approve = {
    deps = [ "specialfs" ];
    text = ''
      if [ -e /boot/dd/unlock/approver.tpm ] && [ -e /dev/tpmrm0 ]; then
        PATH=${lib.makeBinPath tools}:$PATH TPM2TOOLS_TCTI=device:/dev/tpmrm0 \
          ${pkg.dd-boot-key}/bin/dd-boot-key approve-esp ||
          echo "dd-boot-key: the new boot is not approved yet; dd-boot-settle tries again at the next start"
      fi
    '';
  };
  systemd.services.dd-boot-settle = {
    description = "Move the join key's approvals on to this boot";
    wantedBy = [ "multi-user.target" ];
    after = [
      "local-fs.target"
      "tpm2.target"
    ];
    restartIfChanged = false;
    path = tools;
    environment.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
    serviceConfig.Type = "oneshot";
    # a boot not approved, or a TPM not there, is said and left: never a
    # failed unit
    script = ''
      ${pkg.dd-boot-key}/bin/dd-boot-key settle || true
      ${pkg.dd-boot-key}/bin/dd-boot-key approve-esp || true
    '';
  };
}
