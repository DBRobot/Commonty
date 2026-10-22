{ config, lib, ... }:
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
  # the libraries and the mail, copied into this box's own store ahead of
  # the move off the old pool (storage/store.nix migrate)
  dd.store.migrate = lib.genAttrs [ "libraries" "mail" ] (_: {
    envFile = config.sops.templates."library-key.env".path;
    id = "LIBRARY_ID";
    secret = "LIBRARY_SECRET";
  });
  dd.transcode.enable = true;
  # members' locked mail, beside their libraries (modules/library/mail.nix)
  dd.mailInbox.enable = true;
}
