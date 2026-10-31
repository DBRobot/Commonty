# The join key across updates (docs/dev/attestation.md): swtpm, OVMF's
# measured boot through systemd-boot. The box enrols its join key under its
# approver; approves a new boot image (systemd-boot, changed) before
# restarting into it, and its key opens there; settles, which moves the
# epoch on; rolls back to the first boot, still among the good ones, and
# its key opens; a boot it never approved gets nothing and cannot approve
# itself.
{ pkgs, self, ... }:
let
  inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) dd-boot-key dd-control;
  policy = import ../lib/tpm-policy.nix pkgs;
in
{
  name = "boot-key";
  nodes.box = {
    virtualisation = {
      tpm.enable = true;
      useBootLoader = true;
      useEFIBoot = true;
      efi.OVMF = pkgs.OVMFFull.fd;
      memorySize = 1024;
    };
    boot.loader.systemd-boot.enable = true;
    boot.loader.efi.canTouchEfiVariables = true;
    environment.systemPackages = [
      dd-boot-key
      dd-control
      policy
      pkgs.tpm2-tools
    ];
    environment.variables.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
    # dd-control's signer, as the box's heartbeats and joins make one
    environment.etc."control-config.json".text = builtins.toJSON { boxes = { }; };
    # a signature with the join key, as every user of it makes one
    environment.etc."sign.sh".text = ''
      set -euo pipefail
      w=$(mktemp -d)
      trap 'rm -rf "$w"' EXIT
      k=/boot/dd/unlock
      echo hello >"$w/msg"
      tpm2_createprimary -Q -C o -G ecc256 -c "$w/srk.ctx"
      tpm2_load -Q -C "$w/srk.ctx" -u "$k/sig.pub" -r "$k/sig.priv" -c "$w/sig.ctx"
      tpm2_startauthsession -Q --policy-session -S "$w/s.ctx"
      dd-tpm-policy "$w/s.ctx" "$k"
      tpm2_sign -Q -c "$w/sig.ctx" -g sha256 -s ecdsa -p "session:$w/s.ctx" -o "$w/sig" "$w/msg"
    '';
    documentation.enable = false;
  };
  testScript = builtins.readFile ./boot-key.py;
}
