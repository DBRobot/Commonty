# A box's certificate by the DNS Worker, end to end: the real acme module
# (nix/modules/gate/acme.nix) on a box, lego asking the root helper over its
# socket, the helper signing and asking the Worker (run by wrangler), the
# Worker writing to a stand-in Cloudflare that pebble's DNS serves, and
# pebble - Let's Encrypt's own test server - issuing a certificate for the
# box's names and nothing else. The box's TPM is a key file here.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  pebbleCerts = "${pkgs.pebble.src}/test/certs";
  key = pkgs.runCommand "box-key" { nativeBuildInputs = [ pkgs.openssl ]; } ''
    mkdir $out
    openssl ecparam -name prime256v1 -genkey -noout -out ec.pem
    openssl pkcs8 -topk8 -nocrypt -in ec.pem -out $out/key.pem
    openssl ec -in $out/key.pem -pubout -outform DER | base64 -w0 > $out/spki
  '';
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
in
{
  name = "acme";
  helpers = true;
  node.specialArgs = { inherit self; };
  nodes.box1 =
    { config, ... }:
    let
      base = config.dd.domain;
      names = import ../lib/https-names.nix lib config;
      worker = pkgs.runCommand "dns-worker-here" { } ''
        mkdir $out
        # the Worker and every module it imports (controller.js), as deployed
        cp ${self.packages.${pkgs.stdenv.hostPlatform.system}.dns-worker}/* $out/
        chmod -R u+w $out
        cat > $out/fleet.js <<EOF
        export const BOXES = {"box1": "$(cat ${key}/spki)"};
        export const NAMES = {"box1": ${builtins.toJSON names}};
        export const RECORDS = null;
        export const MAIL = null;
        export const MEMBERS = {};
        EOF
        cat > $out/wrangler.toml <<'EOF'
        name = "acme"
        main = "worker.js"
        compatibility_date = "2026-05-01"
        no_bundle = true
        rules = [{ type = "ESModule", globs = ["**/*.js"] }, { type = "CompiledWasm", globs = ["**/*.wasm"] }]
        [vars]
        ZONE = "zone1"
        CF_API = "http://127.0.0.1:8899/client/v4"
        EOF
        printf 'DNS_TOKEN=dns-token\n' > $out/.dev.vars
      '';
    in
    {
      imports = [
        ../modules/box/domain.nix
        ../modules/gate/acme.nix
      ];
      dd.domain = "test.invalid";
      dd.repo = "tester/commonty";
      networking.hostName = "box1";
      virtualisation.memorySize = 2048;
      virtualisation.cores = 2;

      dd.acme.enable = true;
      dd.acme.worker = "http://127.0.0.1:8787";
      dd.acme.softKey = "${key}/key.pem";
      security.acme.defaults.server = "https://localhost:14000/dir";
      # pebble's DNS answers for the names but is no zone's nameserver, so
      # lego's own look at the authoritative servers is left out here
      security.acme.certs.${base} = {
        dnsResolver = "127.0.0.1:8053";
        dnsPropagationCheck = false;
      };
      security.pki.certificateFiles = [ "${pebbleCerts}/pebble.minica.pem" ];
      # boot does not ask: the script does, once all of this is up. The
      # certificate's base unit wants an order too; at boot that one raced
      # wrangler's start, and the script's start joined it and its failure
      systemd.services."acme-order-renew-${base}" = {
        wantedBy = lib.mkForce [ ];
        # lego waits a minute between names and polls every 5s (acme.nix);
        # the record is in pebble's DNS when the helper answers, so poll
        # every second and go straight on. Read after acme.nix's file
        serviceConfig.EnvironmentFile = lib.mkAfter [
          (pkgs.writeText "acme-exec-test.env" ''
            EXEC_POLLING_INTERVAL=1
            EXEC_SEQUENCE_INTERVAL=1
          '')
        ];
      };
      systemd.services."acme-${base}".wants = lib.mkForce [ "acme-setup.service" ];
      systemd.timers."acme-renew-${base}".enable = false;

      services.nginx.virtualHosts = {
        "home.${base}" = {
          useACMEHost = base;
          onlySSL = true;
        };
        "files.${base}" = {
          useACMEHost = base;
          onlySSL = true;
          serverAliases = [ "demo-files.${base}" ];
        };
        # not on the certificate: its own, or none
        "elsewhere.example" = { };
      };

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
        pkgs.curl
        pkgs.jq
        pkgs.openssl
        pkgs.socat
      ];
    };
}
