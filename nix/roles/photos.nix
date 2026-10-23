{ config, lib, ... }:
let
  # this box has ente's key to the old pool
  pool = config.dd.hasSecret "garage-key-id";
in
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
    // lib.optionalAttrs (pool || !(config.dd.failover or false)) {
      # ente's key to the old pool: museum's until failover, and after it
      # what the last copy into the store reads with, until the pool goes.
      # Only a box whose secrets have it: failover may give photos to one
      garage-key-id.owner = "ente";
      garage-key-secret.owner = "ente";
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

  # ente's bucket and key. The key is IMPORTED from sops rather than minted
  # here, so ente is configured with the same credentials at deploy time.
  sops.templates."garage-ente-key.env" = lib.mkIf (pool || !(config.dd.failover or false)) {
    content = ''
      GARAGE_KEY_ID=${config.sops.placeholder.garage-key-id}
      GARAGE_KEY_SECRET=${config.sops.placeholder.garage-key-secret}
    '';
  };
  # the photos, copied into this box's own store ahead of failover; once
  # failover has museum writing the store, once more and then no more, so
  # what was uploaded in the last hour before the switch is there too
  dd.store.migrate.ente = lib.mkIf (pool || !(config.dd.failover or false)) {
    envFile = config.sops.templates."garage-ente-key.env".path;
    id = "GARAGE_KEY_ID";
    secret = "GARAGE_KEY_SECRET";
    once = config.dd.failover or false;
  };
  dd.garage.setupEnvFiles = lib.mkIf (!(config.dd.failover or false)) [
    config.sops.templates."garage-ente-key.env".path
  ];
  dd.garage.buckets.ente = lib.mkIf (!(config.dd.failover or false)) {
    key = {
      name = "ente";
      envPrefix = "GARAGE_KEY";
    };
    allow = [
      "read"
      "write"
      "owner"
    ];
    # Browsers upload blobs straight to garage, so the bucket needs CORS or
    # every upload fails the preflight with "This CORS request is not
    # allowed".
    cors = {
      CORSRules = [
        {
          # Must be "*". Ente decrypts in a web worker, which is a sandboxed
          # context, so the browser sends Origin: null - it cannot send the
          # photos origin even in principle. Listing real origins also makes
          # garage emit them comma-separated, which is invalid and silently
          # rejected. CORS is not the access control here: the presigned URL
          # signature is, and the objects are ciphertext regardless.
          AllowedOrigins = [ "*" ];
          AllowedMethods = [
            "GET"
            "PUT"
            "POST"
            "DELETE"
            "HEAD"
          ];
          AllowedHeaders = [ "*" ];
          ExposeHeaders = [
            "etag"
            "ETag"
            "x-amz-request-id"
          ];
          MaxAgeSeconds = 3000;
        }
      ];
    };
  };
}
