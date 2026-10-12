# The door (servers/smarthome/door): the home controller's only way in. It
# ends TLS for the controller's own name, signs people in against the root
# keys it saved at pairing, and passes each person to Home Assistant from
# their own loopback address (home-assistant.nix). Until it is paired it
# answers the installer on the house network instead (:8080).
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.dd.door;
  door = self.packages.${pkgs.stdenv.hostPlatform.system}.door;
  # the pages as the tree has them (.#pages), the door's among them; files,
  # the same for any machine
  pages = self.packages.x86_64-linux.pages;
  state = "/var/lib/commonty-door";
  dns = "https://dns.${config.dd.domain}";
  # lego's exec provider asks this for each challenge: the controller signs
  # it with its own key, and the DNS Worker writes it only for its own name
  hook = pkgs.writeShellScript "door-acme-hook" ''
    export DOOR_STATE_DIR=${state} DOOR_DNS_WORKER=${dns}
    exec ${door}/bin/dd-door acme-hook "$@"
  '';
  deploy = pkgs.writeShellScript "door-cert-deploy" ''
    install -D -m 0600 "$LEGO_CERT_PATH" ${state}/tls/cert.pem.new
    install -D -m 0600 "$LEGO_CERT_KEY_PATH" ${state}/tls/key.pem.new
    mv ${state}/tls/key.pem.new ${state}/tls/key.pem
    mv ${state}/tls/cert.pem.new ${state}/tls/cert.pem
  '';
in
{
  options.dd.door = {
    enable = lib.mkEnableOption "the door in front of Home Assistant";
    port = lib.mkOption {
      type = lib.types.port;
      default = 443;
      description = "where the door answers on the Commonty network";
    };
    pairPort = lib.mkOption {
      type = lib.types.port;
      default = 8080;
      description = "where the installer pairs with it, on the house network, until it is paired";
    };
    pairKey = lib.mkOption {
      type = lib.types.str;
      default = "/boot/commonty/pair";
      description = "the one-time pairing key the installer wrote to the card";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.dd-door = {
      description = "The door in front of Home Assistant";
      wantedBy = [ "multi-user.target" ];
      after = [
        "network-online.target"
        "home-assistant.service"
      ];
      wants = [ "network-online.target" ];
      path = [ pkgs.systemd ];
      environment = {
        DOOR_STATE_DIR = state;
        DOOR_PAGES_DIR = "${pages}";
        DOOR_LISTEN = "0.0.0.0:${toString cfg.port}";
        DOOR_PAIR_LISTEN = "0.0.0.0:${toString cfg.pairPort}";
        DOOR_PAIR_KEY = cfg.pairKey;
        DOOR_HA = "127.0.0.1:${toString config.dd.smarthome.port}";
        DOOR_USERS_FILE = config.dd.smarthome.usersFile;
        DOOR_NETWORK_KEY = "${state}/network-key";
        # who is a member, as this release says: only a member's name is
        # ever taken here
        DOOR_MEMBERS = "${../../../fleet/members.json}";
        # people's entries, and where the backups go (the gate, for a controller)
        DOOR_GATE = "https://files.${config.dd.domain}";
        # Home Assistant's backups (its backup integration, home-assistant.nix);
        # the door runs as root, reads the newest and removes it once sent
        DOOR_HA_BACKUPS = "${config.services.home-assistant.configDir}/backups";
      };
      serviceConfig = {
        ExecStart = "${door}/bin/dd-door";
        StateDirectory = "commonty-door";
        StateDirectoryMode = "0700";
        Restart = "always";
        RestartSec = 5;
      };
    };

    # once on the network: the name at the DNS Worker (and the address it
    # points at, again whenever it may have moved), then its certificate
    systemd.services.dd-door-name = {
      description = "The home controller's name, at the DNS Worker";
      after = [ "commonty-net-up.service" ];
      restartIfChanged = false;
      environment = {
        DOOR_STATE_DIR = state;
        DOOR_DNS_WORKER = dns;
      };
      serviceConfig.Type = "oneshot";
      script = ''
        # not paired or not joined yet: next time
        [ -s ${state}/registration.json ] && [ -s /run/commonty-net-address ] || exit 0
        ${door}/bin/dd-door register > ${state}/name.new
        mv ${state}/name.new ${state}/name
      '';
    };
    systemd.timers.dd-door-name = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "2m";
        OnUnitActiveSec = "15m";
      };
    };
    systemd.services.dd-door-cert = {
      description = "The home controller's certificate, its key kept here";
      after = [ "dd-door-name.service" ];
      restartIfChanged = false;
      path = [ pkgs.coreutils ];
      environment = {
        EXEC_PATH = "${hook}";
        EXEC_PROPAGATION_TIMEOUT = "300";
        LEGO_ACCEPT_TOS = "true";
      };
      serviceConfig.Type = "oneshot";
      script = ''
        [ -s ${state}/name ] || exit 0
        ${pkgs.lego}/bin/lego run \
          --path ${state}/acme \
          --dns exec \
          --domains "$(cat ${state}/name)" \
          --key-type EC256 \
          --deploy-hook ${deploy}
      '';
    };
    systemd.timers.dd-door-cert = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "5m";
        OnUnitActiveSec = "12h";
      };
    };

    networking.firewall.allowedTCPPorts = [ cfg.pairPort ];
  };
}
