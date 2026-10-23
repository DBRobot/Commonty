{ config, lib, ... }:
{
  imports = [
    ./_sops.nix
    ../modules/photos/ente.nix
    ../modules/storage/ledger.nix
  ];
  sops.secrets =
    lib.optionalAttrs (config.dd.hasSecret "ente-demo-password") {
      # the demo's ente password: it has no passkey to make one from. A box
      # without it has no demo gallery
      ente-demo-password.owner = "dd-verify";
    }
    // lib.optionalAttrs (!(config.dd.failover or false)) {
      # museum's verification code for addresses under users.<domain>: the
      # photos page sends it when it makes a person's account, so it is read
      # by museum (as ente) and by the verifier (as dd-verify), one key twice.
      # With failover on, these are made on the box (ente.nix)
      ente-ott.owner = "ente";
      ente-ott-verify = {
        key = "ente-ott";
        owner = "dd-verify";
      };
      garage-key-id.owner = "ente";
      garage-key-secret.owner = "ente";
      ente-key-encryption.owner = "ente";
      ente-key-hash.owner = "ente";
      ente-jwt-secret.owner = "ente";
    };

  dd.verify.photos = {
    api = "https://api.${config.dd.domain}";
    emailSuffix = "@users.${config.dd.domain}";
    codeFile =
      if (config.dd.failover or false) then
        "/var/lib/dd-ente-keys/ott"
      else
        config.sops.secrets.ente-ott-verify.path;
    demoPasswordFile =
      if config.dd.hasSecret "ente-demo-password" then
        config.sops.secrets.ente-demo-password.path
      else
        null;
  };
  dd.home.services = [
    {
      name = "Photos";
      blurb = "Your photo library, encrypted on your phone before it uploads.";
      # the passkey opens it: the page makes or opens the ente account in
      # the browser and hands ente's app the session
      url = "https://photos.${config.dd.domain}/_dd/photos";
      # the demo has an ente account of its own with a zero quota: ente has
      # no demo mode, a quota is its read-only
      demo = "read";
      # the demo's is a gallery on the gate's public name: Ente's app and
      # server stay on the private network (servers/gate/src/demo_photos.rs)
      demoUrl = "https://home.${config.dd.domain}/_dd/photos/demo";
      icon = "photos";
      color = "#d9822b";
      rank = 10;
    }
  ];
}
