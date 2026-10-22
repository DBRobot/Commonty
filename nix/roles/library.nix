{
  # The members' encrypted libraries: their bucket in this box's own store
  # and the gate in this box's verifier. The gate's keys are the store's,
  # made on the box (modules/library/libraries.nix); nothing else holds them.
  imports = [
    ../modules/library/libraries.nix
    ../modules/library/transcode.nix
    ../modules/library/mail.nix
  ];
  dd.libraries.enable = true;

  dd.transcode.enable = true;
  # members' locked mail, beside their libraries (modules/library/mail.nix)
  dd.mailInbox.enable = true;
}
