# Evaluation only: the box list is well-formed. Every role a box names is a
# file, every box has core, every box builds, and the wiring the flake
# derives from the list (peers, grafana's datasources) says what the list
# says.
{
  pkgs,
  self,
  lib,
  ...
}:
let
  boxes = builtins.fromJSON (builtins.readFile ../../fleet/boxes.json);
  cfgs = self.nixosConfigurations;
  ok = builtins.all (x: x) (
    lib.flatten (
      lib.mapAttrsToList (
        name: box:
        [
          (builtins.elem "core" box.roles)
          (builtins.all (r: builtins.pathExists (../roles + "/${r}.nix")) box.roles)
          (builtins.hasAttr name cfgs)
          (cfgs.${name}.config.networking.hostName == name)
          # peers: every other box, none of itself
          (
            builtins.length cfgs.${name}.config.dd.verify.peers
            == builtins.length (builtins.attrNames boxes) - 1
          )
          (builtins.all (p: !lib.hasInfix box.tailnet p) cfgs.${name}.config.dd.verify.peers)
          # the keyboard way in: a hash nixos will actually write. With
          # mutable users it writes one only for a user it is creating, so
          # the two go together or the console password is decoration
          (cfgs.${name}.config.users.users.admin.hashedPasswordFile != null)
          (cfgs.${name}.config.users.mutableUsers == false)
        ]
        ++ [ (cfgs.${name}.config.services.prometheus.listenAddress == "127.0.0.1") ]
        ++ lib.optionals (builtins.elem "storage" box.roles) [
          # garage: this box's own address is public, its peers never include itself
          (cfgs.${name}.config.dd.garage.publicAddr == "${box.tailnet}:3901")
          (builtins.all (p: !lib.hasInfix box.tailnet p) cfgs.${name}.config.dd.garage.peers)
          (cfgs.${name}.config.dd.garage.zone == box.regionId)
        ]
        ++ lib.optionals (builtins.elem "observe" box.roles) [
          # the fleet view is thanos on this box; no box's prometheus is
          # reachable from another
          (
            builtins.length cfgs.${name}.config.dd.thanos.sidecars == builtins.length (builtins.attrNames boxes)
          )
          # Metrics is our page over Thanos now; grafana, which believed a
          # header from whoever reached it, is gone. Thanos answers reads
          # only, on loopback, and nginx passes it only the query calls.
          (!cfgs.${name}.config.services.grafana.enable)
          (lib.hasPrefix "127.0.0.1:" cfgs.${name}.config.services.thanos.query.http-address)
        ]
        ++ lib.optionals (builtins.elem "llm" box.roles) (
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
in
assert ok;
pkgs.runCommand "box-list" { } "echo ok > $out"
