# Smart home: Home Assistant on the household's home controller (a Raspberry
# Pi in the house, nix/controller), never on a box. It listens on loopback
# alone; the door (servers/smarthome/door) is the only thing on the network
# and the only way in.
#
# Who is who: the door connects to Home Assistant from a loopback address of
# each person's own (127.0.1.<n>), and the trusted networks provider signs
# that address in as that person's Home Assistant user, with no login page.
# The door writes the address-to-user list (usersFile) and restarts Home
# Assistant when it changes. The door's own connections come from
# 127.0.0.1, which is not trusted: it signs in with a password only it holds.
#
# Nothing here reaches the internet for a member: no cloud, no analytics, no
# mobile app integration. A household that adds a cloud integration from
# Home Assistant's settings sends that company what the integration sends.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.smarthome;
  theme =
    (pkgs.runCommand "home-assistant-theme-commonty" { } ''
      mkdir -p $out/themes
      cp ${../../../servers/smarthome/theme/commonty.yaml} $out/themes/commonty.yaml
    '')
    // {
      isHomeAssistantTheme = true;
    };
in
{
  options.dd.smarthome = {
    enable = lib.mkEnableOption "Home Assistant behind the door, on this home controller";
    port = lib.mkOption {
      type = lib.types.port;
      default = 8123;
      description = "Home Assistant's port on 127.0.0.1";
    };
    people = lib.mkOption {
      type = lib.types.str;
      default = "127.0.1.0/24";
      description = "the loopback range the door hands out, one address per person";
    };
    usersFile = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/hass/commonty-users.yaml";
      description = "address -> Home Assistant user id, written by the door";
    };
    keepDays = lib.mkOption {
      type = lib.types.ints.between 1 90;
      default = 30;
      description = "days of history kept (the recorder's purge_keep_days); at most 90";
    };
  };

  config = lib.mkIf cfg.enable {
    services.home-assistant = {
      enable = true;
      # what the house's devices speak, and what the pages need; cloud,
      # analytics and mobile_app are left out on purpose
      extraComponents = [
        "frontend"
        "config"
        "auth"
        "onboarding"
        "automation"
        "script"
        "scene"
        "person"
        "zone"
        "sun"
        "history"
        "logbook"
        "energy"
        "search"
        "backup"
        "zha"
        "matter"
        "thread"
        "otbr"
        "esphome"
        "shelly"
        "hue"
        "homekit_controller"
        "met"
      ];
      themes = [ theme ];
      config = {
        homeassistant = {
          name = "Home";
          auth_providers = [
            {
              type = "trusted_networks";
              trusted_networks = [ cfg.people ];
              trusted_users = "!include ${cfg.usersFile}";
              allow_bypass_login = true;
            }
            # the door's own sign-in, from 127.0.0.1, which is not trusted
            { type = "homeassistant"; }
          ];
        };
        http = {
          server_host = [ "127.0.0.1" ];
          server_port = cfg.port;
          # no use_x_forwarded_for, no trusted_proxies: HA goes by the
          # address a connection comes from (each person's own, from the
          # door), and nothing a request says about itself. Setting either
          # alone is invalid, and HA's http then never starts
        };
        recorder = {
          purge_keep_days = cfg.keepDays;
          # fewer writes: a memory card wears out under the default 5 s
          commit_interval = 30;
        };
        frontend = { };
        config = { };
        automation = "!include automations.yaml";
        script = "!include scripts.yaml";
        scene = "!include scenes.yaml";
        logger.default = "warning";
      };
    };

    systemd.tmpfiles.rules = [
      "f ${config.services.home-assistant.configDir}/automations.yaml 0644 hass hass - []"
      "f ${config.services.home-assistant.configDir}/scripts.yaml 0644 hass hass - {}"
      "f ${config.services.home-assistant.configDir}/scenes.yaml 0644 hass hass - []"
      # nobody until the door lets someone in; an empty map, not a missing file
      "f ${cfg.usersFile} 0644 hass hass - {}"
    ];
    # the people's addresses need no setup: Linux answers all of 127.0.0.0/8
    # on lo, and the door binds each connection's source to one of them
  };
}
