{ config, ... }:
{
  # The members' encrypted libraries: their bucket on this box's garage and
  # the gate in this box's verifier. The gate's key is imported from sops so
  # garage and the verifier agree on it at deploy time; nothing else ever
  # holds it.
  imports = [
    ./_sops.nix
    ../modules/library/libraries.nix
    ../modules/library/transcode.nix
    ../modules/library/mail.nix
  ];
  sops.secrets.library-key-id = { };
  sops.secrets.library-key-secret = { };
  sops.templates."library-key.env".content = ''
    LIBRARY_ID=${config.sops.placeholder.library-key-id}
    LIBRARY_SECRET=${config.sops.placeholder.library-key-secret}
  '';
  dd.libraries = {
    enable = true;
    keyFile = config.sops.templates."library-key.env".path;
  };
  dd.transcode.enable = true;
  # members' locked mail, beside their libraries (modules/library/mail.nix)
  dd.mailInbox.enable = true;
}
