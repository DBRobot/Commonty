# dd-tpm-policy (nix/modules/box/tpm-policy.sh): the policy session every
# user of the box's TPM-held join key starts with: `import ./tpm-policy.nix pkgs`
pkgs:
pkgs.writeShellApplication {
  name = "dd-tpm-policy";
  runtimeInputs = [
    pkgs.tpm2-tools
    pkgs.coreutils
  ];
  text = builtins.readFile ../modules/box/tpm-policy.sh;
}
