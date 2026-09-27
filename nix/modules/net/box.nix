# A box on the fleet's own network: a second tailscaled beside the one on
# the owner's Tailscale account, against the fleet's Headscale, with its
# own state, socket, port and interface. Both run until the fleet's names
# and every address have moved over and the old one is retired by hand.
# Names on this network come from Headscale; the box keeps its own
# resolver and takes none from here (accept-dns off).
#
# How a box reaches the control server. Not by the control box's tailnet
# address: a tailscaled steers its own control traffic around every
# tailnet, the owner's included, so on a box already on the owner's
# tailnet that address has no route and the dial times out - node2 sat
# logged out for days on exactly that. Through the front door instead,
# like everything else a box reaches, with the app's bridge in front of
# the stock daemon: the front door's proxy carries websockets but strips
# the control protocol's own upgrade, and the bridge turns one into the
# other. The control box runs the control server, so it talks to itself.
{
  config,
  pkgs,
  lib,
  self,
  ...
}:
let
  cfg = config.dd.net;
  base = config.dd.domain;
  bridged = !config.services.headscale.enable;
  bridgeAt = "127.0.0.1:41643";
in
{
  options.dd.net = {
    enable = lib.mkEnableOption "this box on the fleet's own network";
    keyFile = lib.mkOption {
      type = lib.types.str;
      description = "file holding the boxes' join key (made by headscale-seed on the control box; carried to the others through sops)";
    };
    controlUrl = lib.mkOption {
      type = lib.types.str;
      default = "https://headscale.${base}";
      description = "the fleet's control server";
    };
    socket = lib.mkOption {
      type = lib.types.str;
      default = "/run/commonty-net/tailscaled.sock";
      readOnly = true;
    };
    controlAddress = lib.mkOption {
      type = lib.types.str;
      description = "the control box's tailnet address: on the control box itself, where the control server's name resolves to. Other boxes go through the front door.";
    };
    afterJoin = lib.mkOption {
      type = lib.types.lines;
      default = "";
      description = "shell run once this box is on the network (the control box writes the fleet's names)";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.commonty-net = {
      description = "The fleet's own network: tailscaled against the fleet's Headscale";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = [
        pkgs.iproute2
        pkgs.procps
        pkgs.getent
        pkgs.kmod
      ];
      serviceConfig = {
        ExecStart = ''
          ${pkgs.tailscale}/bin/tailscaled \
            --state=/var/lib/commonty-net/tailscaled.state \
            --socket=${cfg.socket} \
            --port=41642 \
            --tun=commonty0 \
            --no-logs-no-support
        '';
        StateDirectory = "commonty-net";
        StateDirectoryMode = "0700";
        RuntimeDirectory = "commonty-net";
        Restart = "on-failure";
        RestartSec = 5;
      };
    };

    # Joined with the boxes' key; the key carries the tag the policy grants
    # to, so the node asks for none. A keeper, not a oneshot: it tries
    # until the control server answers and then runs what comes after,
    # and a release switch never waits on it or fails because of it (a
    # box whose control server is down still takes releases). The owner's
    # tailscale keeps the firewall, this one stays out of netfilter.
    systemd.services.commonty-net-bridge = lib.mkIf bridged {
      description = "The fleet's control server, through the front door";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      serviceConfig = {
        ExecStart = "${
          self.packages.${pkgs.stdenv.hostPlatform.system}.netbridge
        }/bin/netbridge -listen ${bridgeAt} -control ${cfg.controlUrl}";
        DynamicUser = true;
        Restart = "always";
        RestartSec = 5;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
        ];
        CapabilityBoundingSet = "";
      };
    };

    systemd.services.commonty-net-up = {
      description = "Join the fleet's own network";
      after = [ "commonty-net.service" ] ++ lib.optional bridged "commonty-net-bridge.service";
      requires = [ "commonty-net.service" ] ++ lib.optional bridged "commonty-net-bridge.service";
      wantedBy = [ "multi-user.target" ];
      path = [
        pkgs.tailscale
        pkgs.jq
      ];
      serviceConfig = {
        Type = "simple";
        Restart = "on-failure";
        RestartSec = 30;
      };
      script = ''
        set -u
        sock=${cfg.socket}
        for _ in $(seq 1 30); do
          tailscale --socket "$sock" status >/dev/null 2>&1 && break
          sleep 1
        done
        wait=30
        until tailscale --socket "$sock" status --json 2>/dev/null | jq -e '.BackendState == "Running"' >/dev/null; do
          if [ ! -s ${cfg.keyFile} ]; then
            echo "no key at ${cfg.keyFile} yet"
            sleep 30
            continue
          fi
          tailscale --socket "$sock" up \
            --login-server=${if bridged then "http://${bridgeAt}" else cfg.controlUrl} \
            --auth-key=file:${cfg.keyFile} \
            --hostname=${config.networking.hostName} \
            --accept-dns=false \
            --accept-routes=false \
            --netfilter-mode=off \
            --timeout=60s && continue
          # Every `up` rewrites routing table 52, which the box's other
          # tailscaled shares. Retrying one that fails, every thirty
          # seconds, is what took node2 off the network for eight hours.
          # Back off instead: a box that cannot join is a box to look at,
          # not one to keep poking.
          wait=$(( wait * 2 ))
          [ $wait -gt 3600 ] && wait=3600
          echo "could not join; next try in $wait s"
          sleep $wait
        done
        echo "on the network as $(tailscale --socket "$sock" ip -4)"
        # where this box is on the network, for pages that tell people where
        # to connect (the games manager); moved into place, never half written
        tailscale --socket "$sock" ip -4 > /run/commonty-net-address.tmp
        mv /run/commonty-net-address.tmp /run/commonty-net-address
        ${cfg.afterJoin}
      '';
    };

    # the control box reaches its own control server by name
    networking.hosts = lib.mkIf (!bridged) { ${cfg.controlAddress} = [ "headscale.${base}" ]; };

    networking.firewall.allowedUDPPorts = [ 41642 ];
    networking.networkmanager.unmanaged = [ "interface-name:commonty0" ];
  };
}
