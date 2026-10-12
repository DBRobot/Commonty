# A box on the fleet's own network: the box's one tailscaled, against the
# fleet's Headscale, with its own state, socket, port and interface
# (commonty0); accept-dns off. It reaches the control server through the
# front door with the app's bridge in front (the proxy strips the control
# protocol's upgrade); the control box talks to itself.
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
  # with failover, the control server may be on any box able to run it, or
  # none: every box asks through the front door as any other does
  bridged = !config.services.headscale.enable || (config.dd.failover or false);
  bridgeAt = "127.0.0.1:41643";
  control = self.packages.${pkgs.stdenv.hostPlatform.system}.dd-control;
in
{
  options.dd.net = {
    enable = lib.mkEnableOption "this box on the fleet's own network";
    getKey = lib.mkOption {
      type = lib.types.lines;
      description = "shell that writes a single-use join key to $key. A box asks its gate, signed by its TPM, and the answer comes sealed to its TPM; the control box mints its own (modules/net/headscale.nix).";
    };
    door = lib.mkOption {
      type = lib.types.str;
      default = "https://home.${base}/_dd/network/box";
      description = "where a box asks for its key";
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
    afterJoin = lib.mkOption {
      type = lib.types.lines;
      default = "";
      description = "shell run once this box is on the network (the control box writes the fleet's names)";
    };
  };

  config = lib.mkIf cfg.enable {
    # asked of the gate: the time, signed in this box's TPM; the answer
    # opens only with the secrets key in the same TPM
    dd.net.getKey = lib.mkDefault ''
      ${control}/bin/dd-control -box ${config.networking.hostName} -sign-join \
        | curl -fsS --max-time 60 -H 'content-type: application/json' --data-binary @- ${cfg.door} \
        | age -d -i /var/lib/dd/secrets-key.txt > "$key"
    '';
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
    # box whose control server is down still takes releases). It stays out
    # of netfilter: the box's own firewall rules are core.nix's.
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
        pkgs.curl
        pkgs.age
        pkgs.age-plugin-tpm
        pkgs.tpm2-tools
        pkgs.coreutils
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
          # a key for this one try: single use, ten minutes, never kept
          key=/run/commonty-net/join.key
          rm -f "$key"
          if ! { ${cfg.getKey}
          } || [ ! -s "$key" ]; then
            rm -f "$key"
            echo "no key to join with; next try in $wait s"
            sleep $wait
            wait=$(( wait * 2 ))
            [ $wait -gt 3600 ] && wait=3600
            continue
          fi
          tailscale --socket "$sock" up \
            --login-server=${if bridged then "http://${bridgeAt}" else cfg.controlUrl} \
            --auth-key=file:"$key" \
            --hostname=${config.networking.hostName} \
            --accept-dns=false \
            --accept-routes=false \
            --netfilter-mode=off \
            --timeout=60s && { rm -f "$key"; continue; }
          rm -f "$key"
          # every `up` rewrites routing table 52; retrying each thirty seconds
          # once took node2 off the network for eight hours, so back off
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

    # the control box reaches its own control server by name, on itself:
    # nginx answers the name on every address, loopback included
    networking.hosts = lib.mkIf (!bridged) { "127.0.0.1" = [ "headscale.${base}" ]; };

    networking.firewall.allowedUDPPorts = [ 41642 ];
    # The Commonty network reaches this box's services and nothing past it:
    # the box forwards (docker), and a member who
    # sent a packet for a house address (192.168.1.x) to this box would
    # otherwise be let into the house. Nothing is forwarded in or out on
    # commonty0, whatever routes anyone adds.
    networking.firewall.extraCommands = ''
      for ipt in iptables ip6tables; do
        $ipt -D FORWARD -i commonty0 -j DROP 2>/dev/null || true
        $ipt -D FORWARD -o commonty0 -j DROP 2>/dev/null || true
        $ipt -I FORWARD 1 -i commonty0 -j DROP
        $ipt -I FORWARD 1 -o commonty0 -j DROP
      done
    '';
    networking.firewall.extraStopCommands = ''
      for ipt in iptables ip6tables; do
        $ipt -D FORWARD -i commonty0 -j DROP 2>/dev/null || true
        $ipt -D FORWARD -o commonty0 -j DROP 2>/dev/null || true
      done
    '';
    networking.networkmanager.unmanaged = [ "interface-name:commonty0" ];
  };
}
