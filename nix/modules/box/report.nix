# This box's report on itself, made at every boot in /var/lib/dd/report and
# signed by its TPM: hardware-configuration.nix (nixos-generate-config's,
# unedited), hardware.json (report/detect.sh) and report.sig. The owner
# copies it into fleet/boxes/<box>/ with scripts/box-report; the box-reports
# check refuses a folder the box did not sign.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  pkg = self.packages.${pkgs.stdenv.hostPlatform.system};
  out = "/var/lib/dd/report";
in
{
  config = lib.mkIf config.dd.unlock.enable {
    systemd.services.dd-box-report = {
      description = "This box's signed report on its own hardware";
      wantedBy = [ "multi-user.target" ];
      after = [ "local-fs.target" ];
      path = [
        pkgs.bash
        pkgs.util-linux
        pkgs.coreutils
        pkgs.tpm2-tools
        config.system.build.nixos-generate-config
        pkg.dd-control
      ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        set -eu
        w=$(mktemp -d)
        trap 'rm -rf "$w"' EXIT
        nixos-generate-config --no-filesystems --show-hardware-config > "$w/hardware-configuration.nix"
        bash ${./report/detect.sh} > "$w/hardware.json"
        dd-control -sign-report "$w" -box ${config.networking.hostName}
        mkdir -p ${out}
        cp "$w"/hardware-configuration.nix "$w"/hardware.json "$w"/report.sig ${out}/
        echo "report signed"
      '';
    };
  };
}
