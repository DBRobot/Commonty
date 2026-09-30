{ config, ... }:
{
  imports = [
    ./_sops.nix
    ../modules/photos/ente.nix
  ];
  sops.secrets = {
    # museum's verification code for addresses under users.<domain>: the
    # photos page sends it when it makes a person's account, so it is read
    # by museum (as ente) and by the verifier (as dd-verify), one key twice
    ente-ott.owner = "ente";
    ente-ott-verify = {
      key = "ente-ott";
      owner = "dd-verify";
    };
    # the demo's ente password: it has no passkey to make one from
    ente-demo-password.owner = "dd-verify";
    garage-key-id.owner = "ente";
    garage-key-secret.owner = "ente";
    ente-key-encryption.owner = "ente";
    ente-key-hash.owner = "ente";
    ente-jwt-secret.owner = "ente";
    ente-smtp-password.owner = "ente";
  };

  dd.verify.photos = {
    api = "https://api.${config.dd.domain}";
    emailSuffix = "@users.${config.dd.domain}";
    codeFile = config.sops.secrets.ente-ott-verify.path;
    demoPasswordFile = config.sops.secrets.ente-demo-password.path;
  };
  # museum's id for the owner's photo account; the forge's admin is named
  # the same way in forge.nix
  dd.photos.admin = 1580559962386438;

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
      # server stay on the private network (box/verify/src/demo_photos.rs)
      demoUrl = "https://home.${config.dd.domain}/_dd/photos/demo";
      icon = "photos";
      color = "#d9822b";
      rank = 10;
    }
  ];

  # ente's bucket and key. The key is IMPORTED from sops rather than minted
  # here, so ente is configured with the same credentials at deploy time.
  sops.templates."garage-ente-key.env".content = ''
    GARAGE_KEY_ID=${config.sops.placeholder.garage-key-id}
    GARAGE_KEY_SECRET=${config.sops.placeholder.garage-key-secret}
  '';
  dd.garage.setupEnvFiles = [ config.sops.templates."garage-ente-key.env".path ];
  dd.garage.buckets.ente = {
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
