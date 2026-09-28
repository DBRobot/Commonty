# Two boxes measure themselves; the observe box answers for both. Each
# sidecar reads its own prometheus and holds a key to the metrics bucket in
# the cluster the two boxes form; the query on a spans a's blocks, a's
# sidecar and b's sidecar, and keeps answering when b is gone.
{ pkgs, self, ... }:
let
  rpc = pkgs.writeText "garage.env" "GARAGE_RPC_SECRET=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
  key = box: {
    id = "GK${builtins.substring 0 24 (builtins.hashString "sha256" box)}";
    secret = builtins.hashString "sha256" "${box}-secret";
  };
  objstore =
    box:
    pkgs.writeText "objstore.yaml" ''
      type: S3
      config:
        bucket: metrics
        endpoint: 127.0.0.1:3900
        region: us-east-1
        access_key: ${(key box).id}
        secret_key: ${(key box).secret}
        insecure: true
        signature_version2: false
        bucket_lookup_type: path
    '';
  box = name: {
    # the gate plays no part here: without it, a change to the gate
    # is not a reason to run this again
    dd.verify.enable = false;
    imports = [
      ./box.nix
      ../modules/storage/garage.nix
      ../modules/observe/thanos.nix
    ];
    dd.garage = {
      zone = "r-test";
      capacity = "1G";
      dataDir = "/srv/garage";
      publicAddr = "${name}:3901";
      envFile = rpc;
      setupEnvFiles = [
        (pkgs.writeText "metrics-key.env" ''
          GARAGE_METRICS_KEY_ID=${(key name).id}
          GARAGE_METRICS_KEY_SECRET=${(key name).secret}
        '')
      ];
      buckets.metrics.key = {
        name = "metrics-${name}";
        envPrefix = "GARAGE_METRICS_KEY";
      };
    };
    dd.thanos.objstoreFile = toString (objstore name);
    environment.systemPackages = [ pkgs.curl ];
  };
in
{
  name = "thanos";
  node.specialArgs = { inherit self; };
  defaults.virtualisation.memorySize = 2048;
  defaults.virtualisation.cores = 2; # services start in parallel instead of queueing on one thread
  defaults.virtualisation.diskSize = 4096;
  nodes = {
    a = {
      imports = [
        (box "a")
        {
          # by address, as the fleet does (tailnet ips): the test net has
          # no dns, and grpc's resolver never settles on a bare hostname
          dd.thanos.sidecars = [
            "192.168.1.1:10901"
            "192.168.1.2:10901"
          ];
        }
      ];
    };
    b = box "b";
  };
  scriptEnv = {
    inherit rpc;
  };
}
