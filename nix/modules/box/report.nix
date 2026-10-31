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
  key = "/var/lib/dd/secrets-key.txt";
in
{
  config = lib.mkIf config.dd.unlock.enable {
    # The key this box's secrets are encrypted to (secrets/<box>.yaml): made
    # in its TPM, once, and never leaving it; the file is a handle only this
    # TPM can use, on the encrypted root. Its public half is in the report.
    systemd.services.dd-secrets-key = {
      description = "This box's secrets key, in its TPM";
      wantedBy = [ "multi-user.target" ];
      unitConfig.ConditionPathExists = "!${key}";
      path = [ pkgs.age-plugin-tpm ];
      serviceConfig.Type = "oneshot";
      script = ''
        umask 077
        mkdir -p /var/lib/dd
        age-plugin-tpm --generate -o ${key}.new
        mv ${key}.new ${key}
      '';
    };

    # A box the release does not list (dd.box.joined): its key to join the
    # fleet with, made in its TPM as dd-unlock-enrol makes a listed box's
    # (usable only under its signed boot), its secrets key's recipient,
    # and its own report where dd-agent builds its system from
    # (docs/dev/joining.md)
    systemd.services.dd-join-prepare = lib.mkIf config.dd.box.joined {
      description = "This box's key to join the fleet, and its report to build from";
      wantedBy = [ "multi-user.target" ];
      # a box on its TPM alone makes its key at its first boot, after its
      # disk's key is sealed (disk-unlock-seal.sh)
      after = [
        "dd-box-report.service"
      ]
      ++ lib.optional config.dd.unlock.tpmOnly "dd-unlock-seal.service";
      requires = [ "dd-box-report.service" ];
      before = [ "dd-control.service" ];
      requiredBy = [ "dd-control.service" ];
      path = [
        pkgs.tpm2-tools
        pkgs.coreutils
        pkgs.jq
      ];
      environment.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        set -eu
        K=/boot/dd/unlock
        w=$(mktemp -d)
        trap 'rm -rf "$w"' EXIT
        mkdir -p $K
        tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
        sb=$(od -An -tu1 -j4 -N1 /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c 2>/dev/null | tr -d ' ' || true)
        if [ ! -s $K/sig.pub ] && [ "$sb" != 1 ]; then
          # a key made under any boot but a signed one would sign for any
          echo "no key to join with until Secure Boot is on"
          exit 1
        fi
        if [ ! -s $K/sig.pub ]; then
          # under its approver: PCRs 4 and 7 of the boots it approves
          # (docs/dev/attestation.md), what a join's proof asks for
          ${pkg.dd-boot-key}/bin/dd-boot-key enrol -keys $K -dir "$w/bk"
          echo "this box's key to join the fleet, made in its TPM"
        fi
        tpm2_load -Q -C "$w/srk.ctx" -u $K/sig.pub -r $K/sig.priv -c "$w/sig.ctx"
        tpm2_readpublic -Q -c "$w/sig.ctx" -f der -o "$w/spki.der"
        tpm2_flushcontext -t >/dev/null 2>&1 || true
        umask 022
        base64 -w0 < "$w/spki.der" > /var/lib/dd/box-key.spki.new
        mv /var/lib/dd/box-key.spki.new /var/lib/dd/box-key.spki
        jq -er .secretsKey ${out}/hardware.json > /var/lib/dd/secrets-recipient.new
        mv /var/lib/dd/secrets-recipient.new /var/lib/dd/secrets-recipient
        mkdir -p /var/lib/dd/box
        cp ${out}/hardware.json ${out}/hardware-configuration.nix /var/lib/dd/box/
        # what an owner would choose, nobody has: wired, its address from
        # the router, its disks as they are
        [ -s /var/lib/dd/box/box.json ] || jq -n '{owner: null, house: {ssid: "", address: ""},
          encrypted: false, extraPools: [], datasets: {}}' > /var/lib/dd/box/box.json
      '';
    };

    systemd.services.dd-box-report = {
      description = "This box's signed report on its own hardware";
      wantedBy = [ "multi-user.target" ];
      after = [
        "local-fs.target"
        "dd-secrets-key.service"
        # the links as the box uses them (report/nics.sh)
        "NetworkManager.service"
      ];
      wants = [ "dd-secrets-key.service" ];
      path = [
        pkgs.bash
        pkgs.util-linux
        pkgs.coreutils
        pkgs.tpm2-tools
        pkgs.age-plugin-tpm
        pkgs.jq
        pkgs.gawk
        pkgs.libva-utils
        # the Wi-Fi bands, and how the links are used (report/nics.sh)
        pkgs.iw
        pkgs.gnused
        config.networking.networkmanager.package
        config.boot.zfs.package
        config.system.build.nixos-generate-config
        pkg.dd-control
        # its signature is the join key's, in the session dd-tpm-policy makes
        (import ../../lib/tpm-policy.nix pkgs)
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
        SECRETS_KEY=${key} INITRD_MODULES="${lib.concatStringsSep " " config.boot.initrd.availableKernelModules}" \
          LIBVA_DRIVERS_PATH=/run/opengl-driver/lib/dri TPM2TOOLS_TCTI=device:/dev/tpmrm0 \
          bash ${./report}/detect.sh > "$w/hardware.json"
        dd-control -sign-report "$w" -box ${config.networking.hostName}
        mkdir -p ${out}
        cp "$w"/hardware-configuration.nix "$w"/hardware.json "$w"/report.sig ${out}/
        echo "report signed"
      '';
    };
  };
}
