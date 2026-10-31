# A copy into a joined box's store, over the store's port with TLS as on the
# fleet (store.nix's own test has no certificate): `a`, a box of the release,
# copies into `j`, a box that joined on its own key. Its certificate comes by
# the real acme module, the DNS Worker under wrangler with a stand-in for the
# agreed state and for Cloudflare, and pebble. Until the agreed state has its
# key it has only nginx's self-signed one, and the copy fails on TLS.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  name = "j0123456789";
  storeName = "store.${name}.test.invalid";
  pebbleCerts = "${pkgs.pebble.src}/test/certs";
  key = pkgs.runCommand "joined-key" { nativeBuildInputs = [ pkgs.openssl ]; } ''
    mkdir $out
    openssl ecparam -name prime256v1 -genkey -noout -out ec.pem
    openssl pkcs8 -topk8 -nocrypt -in ec.pem -out $out/key.pem
    openssl ec -in $out/key.pem -pubout -outform DER | base64 -w0 > $out/spki
  '';
  # test keys only, as store.nix's: on a real box its TPM-held secrets key
  ages = {
    a = {
      identity = "AGE-SECRET-KEY-1RYQV6YMT4DPUURXLGDHLEP6P4QLM4NYQ4Z4E594SKSVYTXHZTF6QAJ94TJ";
      recipient = "age1gc25m409smgcklvytkksgew4ycjky4xtwvarsynwjxa9eg60jgpqfrlgp8";
    };
    j = {
      identity = "AGE-SECRET-KEY-18PLY0LS6RNEJLKPAYQRU6FD35UMDPS7WLXV94ASZ4G286NWRKSJQQPJL2V";
      recipient = "age1qg66feuknweacfxtff0mfejky0xzuk8d23zssz763lpmptxxquessgd8hd";
    };
  };
  pebbleConfig = pkgs.writeText "pebble.json" (
    builtins.toJSON {
      pebble = {
        listenAddress = "127.0.0.1:14000";
        managementListenAddress = "127.0.0.1:15000";
        certificate = "${pebbleCerts}/localhost/cert.pem";
        privateKey = "${pebbleCerts}/localhost/key.pem";
        httpPort = 5002;
        tlsPort = 5001;
        ocspResponderURL = "";
        externalAccountBindingRequired = false;
      };
    }
  );
  worker = pkgs.runCommand "dns-worker-here" { } ''
    mkdir $out
    cp ${self.packages.${pkgs.stdenv.hostPlatform.system}.dns-worker}/* $out/
    chmod -R u+w $out
    cat > $out/fleet.js <<EOF
    export const BOXES = {};
    export const NAMES = {};
    export const RECORDS = {"zone": "test.invalid", "address": "100.64.0.2", "tunnel": "t1", "hosts": ["home"], "open": false, "bare": false, "home": "a", "gateway": "headscale", "addresses": {"a": "100.64.0.2"}, "fleet": ["100.64.0.2"], "follow": {"games": "dd-games"}, "leased": ["headscale"]};
    export const MAIL = null;
    export const MEMBERS = {};
    EOF
    cat > $out/wrangler.toml <<'TOML'
    name = "joined-store-tls"
    main = "worker.js"
    compatibility_date = "2026-05-01"
    no_bundle = true
    rules = [{ type = "ESModule", globs = ["**/*.js"] }, { type = "CompiledWasm", globs = ["**/*.wasm"] }]
    [vars]
    ZONE = "zone1"
    CF_API = "http://127.0.0.1:8899/client/v4"
    CONTROL_URL = "http://127.0.0.1:8899/control"
    TOML
    sed -i 's/^    //' $out/fleet.js $out/wrangler.toml
    printf 'DNS_TOKEN=dns-token\n' > $out/.dev.vars
  '';
in
{
  name = "joined-store-tls";
  helpers = true;
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.diskSize = 2048;
  nodes.a = {
    imports = [
      ./box.nix
      ../modules/storage/store.nix
    ];
    dd.verify.enable = false;
    dd.store = {
      enable = true;
      capacity = "1G";
      identity = "/etc/age-identity";
      agePlugins = [ ];
      buckets.photos = { };
    };
    environment.etc."age-identity" = {
      text = ages.a.identity;
      mode = "0400";
    };
    # its own store's name is not what is under test
    services.nginx.enable = true;
    services.nginx.virtualHosts."store.a.test.invalid" = {
      useACMEHost = lib.mkForce null;
      onlySSL = lib.mkForce false;
      listen = lib.mkForce [
        {
          addr = "0.0.0.0";
          port = 3443;
        }
      ];
    };
    networking.extraHosts = "192.168.1.2 ${storeName}";
    # pebble's root, which the test hands it, as a box trusts Let's Encrypt
    systemd.services.dd-store-copy.environment = {
      CURL_CA_BUNDLE = "/run/test-ca.pem";
      SSL_CERT_FILE = "/run/test-ca.pem";
    };
    systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
    environment.systemPackages = [
      pkgs.awscli2
      pkgs.curl
      pkgs.python3
    ];
  };
  nodes.j =
    { config, ... }:
    let
      base = config.dd.domain;
    in
    {
      imports = [
        ./box.nix
        ../modules/storage/store.nix
        ../modules/gate/acme.nix
      ];
      dd.verify.enable = false;
      networking.hostName = lib.mkForce name;
      dd.box.joined = true;
      dd.store = {
        enable = true;
        capacity = "1G";
        identity = "/etc/age-identity";
        agePlugins = [ ];
        peers.a = {
          url = "http://store.a.test.invalid:3443";
          inherit (ages.a) recipient;
        };
        buckets.photos = { };
      };
      environment.etc."age-identity" = {
        text = ages.j.identity;
        mode = "0400";
      };
      systemd.tmpfiles.rules = [ "d /run/dd-placed 0755 root root -" ];
      # a vhost its nginx has that is no name of its own, as the real
      # joined box's home.<domain>: it must not be on the order
      services.nginx.virtualHosts."home.${base}" = {
        useACMEHost = base;
        onlySSL = true;
      };

      dd.acme.enable = true;
      dd.acme.worker = "http://127.0.0.1:8787";
      dd.acme.softKey = "${key}/key.pem";
      security.acme.defaults.server = "https://localhost:14000/dir";
      security.acme.certs.${base} = {
        dnsResolver = "127.0.0.1:8053";
        dnsPropagationCheck = false;
      };
      security.pki.certificateFiles = [ "${pebbleCerts}/pebble.minica.pem" ];
      # the test asks, once all of this is up
      systemd.timers."acme-order-renew-${base}-after-switch".enable = false;
      systemd.timers."acme-renew-${base}".enable = false;
      systemd.services."acme-order-renew-${base}".serviceConfig.EnvironmentFile = lib.mkAfter [
        (pkgs.writeText "acme-exec-test.env" ''
          EXEC_POLLING_INTERVAL=1
          EXEC_SEQUENCE_INTERVAL=1
        '')
      ];
      systemd.services = {
        challtestsrv = {
          wantedBy = [ "multi-user.target" ];
          serviceConfig.ExecStart = "${pkgs.pebble}/bin/pebble-challtestsrv -defaultIPv4 127.0.0.1 -defaultIPv6 '' -dns01 127.0.0.1:8053 -management 127.0.0.1:8055 -http01 '' -https01 '' -tlsalpn01 '' -doh ''";
        };
        pebble = {
          wantedBy = [ "multi-user.target" ];
          after = [ "challtestsrv.service" ];
          environment = {
            PEBBLE_VA_NOSLEEP = "1";
            PEBBLE_WFE_NONCEREJECT = "0";
          };
          serviceConfig.ExecStart = "${pkgs.pebble}/bin/pebble -config ${pebbleConfig} -dnsserver 127.0.0.1:8053";
        };
        fake-cloudflare = {
          wantedBy = [ "multi-user.target" ];
          environment.CHALLTESTSRV = "http://127.0.0.1:8055";
          serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${./dns-worker/fake.py}";
        };
        dns-worker = {
          wantedBy = [ "multi-user.target" ];
          environment = {
            HOME = "/tmp";
            WRANGLER_SEND_METRICS = "false";
            CI = "1";
            CLOUDFLARE_CF_FETCH_ENABLED = "false";
          };
          serviceConfig = {
            WorkingDirectory = "/run/dns-worker";
            ExecStartPre = "${pkgs.bash}/bin/sh -c 'cp -rL --no-preserve=mode ${worker}/. /run/dns-worker/'";
            ExecStart = "${pkgs.wrangler}/bin/wrangler dev --port 8787 --ip 127.0.0.1";
            RuntimeDirectory = "dns-worker";
            PrivateTmp = true;
          };
        };
      };
      environment.systemPackages = [
        pkgs.awscli2
        pkgs.curl
        pkgs.jq
        pkgs.openssl
        pkgs.socat
        pkgs.tpm2-tools
      ];
    };
  scriptEnv = {
    inherit name storeName;
    spki = "${key}/spki";
    softKey = "${key}/key.pem";
    ask = ../modules/gate/acme-ask.sh;
  };
}
