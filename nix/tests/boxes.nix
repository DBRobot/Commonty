# Evaluation only: the box list is well-formed. No box names a role (what
# it is built with comes from its capability, nix/lib/fleet-placement.nix),
# every box is built with core, every box builds, and the wiring the flake
# derives (peers, garage's addresses, thanos on its own box) says what the
# boxes are built with.
{
  pkgs,
  self,
  lib,
  ...
}:
let
  boxes = builtins.fromJSON (builtins.readFile ../../fleet/boxes.json);
  fp = import ../lib/fleet-placement.nix { inherit lib; };
  has = name: s: builtins.elem s fp.built.${name};
  cfgs = self.nixosConfigurations;
  ok = builtins.all (x: x) (
    lib.flatten (
      lib.mapAttrsToList (
        name: box:
        [
          (!(box ? roles))
          (builtins.elem "core" fp.carried.${name})
          (builtins.all (r: builtins.pathExists (../roles + "/${r}.nix")) fp.carried.${name})
          (builtins.hasAttr name cfgs)
          (cfgs.${name}.config.networking.hostName == name)
          # every box has its address on the fleet's network recorded
          (box ? net && cfgs.${name}.config.dd.box.net == box.net)
          # the keyboard way in: a hash nixos will actually write. With
          # mutable users it writes one only for a user it is creating, so
          # the two go together or the console password is decoration
          (cfgs.${name}.config.users.users.admin.hashedPasswordFile != null)
          (cfgs.${name}.config.users.mutableUsers == false)
        ]
        ++ [ (cfgs.${name}.config.services.prometheus.listenAddress == "127.0.0.1") ]
        ++ lib.optionals (has name "garage") [
          # garage: this box's own address is public, its peers never include itself
          (cfgs.${name}.config.dd.garage.publicAddr == "${box.net}:3901")
          (builtins.all (p: !lib.hasInfix box.net p) cfgs.${name}.config.dd.garage.peers)
          (cfgs.${name}.config.dd.garage.zone == box.regionId)
          (lib.hasPrefix "127.0.0.1:" cfgs.${name}.config.services.thanos.sidecar.grpc-address)
        ]
        ++ lib.optionals (has name "thanos-query") [
          # the fleet view is thanos on this box: its own sidecar and the
          # bucket, never another box's sidecar (README: no box talks to
          # another), and every sidecar answers its own box alone
          (cfgs.${name}.config.dd.thanos.sidecars == [ "127.0.0.1:10901" ])
          # Metrics is our page over Thanos now; grafana, which believed a
          # header from whoever reached it, is gone. Thanos answers reads
          # only, on loopback, and nginx passes it only the query calls.
          (!cfgs.${name}.config.services.grafana.enable)
          (lib.hasPrefix "127.0.0.1:" cfgs.${name}.config.services.thanos.query.http-address)
        ]
        ++ lib.optionals (has name "llama-swap") (
          let
            c = cfgs.${name}.config;
          in
          [
            # a model server answers whoever reaches it: llama-swap and the
            # servers it starts have a network of their own, which nothing
            # else on the box shares (modules/llm/llama-cpp.nix)
            (c.systemd.services.llama-swap.serviceConfig.PrivateNetwork or false)
            # and llama-swap still wants the key only nginx holds
            (c.systemd.services.llama-swap.serviceConfig ? EnvironmentFile)
            # nginx's way in is a socket its group alone may open
            (c.systemd.sockets.llm-proxy.socketConfig.SocketGroup == "nginx")
            (c.systemd.sockets.llm-proxy.socketConfig.SocketMode == "0660")
          ]
        )
      ) boxes
    )
  );
  # two services on one port fail the build (modules/box/ports.nix): a
  # box made to claim the transcoder's port for something else does not
  clash =
    (builtins.tryEval
      (cfgs.node1.extendModules { modules = [ { dd.ports.impostor = [ 4190 ]; } ]; })
      .config.system.build.toplevel.drvPath
    ).success;
in
assert ok;
assert !clash;
pkgs.runCommand "box-list" { } "echo ok > $out"
