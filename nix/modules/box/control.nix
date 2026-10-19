# The control plane on this box (servers/system/control, docs/dev/control-plane.md):
# dd-control heartbeats what the box has and runs, signed by its TPM, and
# keeps the agreed state at http://127.0.0.1:4195/state, checked against the
# validators' signatures. A box named as its house's validator in
# fleet/control.json also votes. It dials out to the relay at
# control.<domain> and nothing dials in. It acts on the state for the
# placed services this box carries (box/placed.nix), and nothing else.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.dd.control;
  pkg = self.packages.${pkgs.stdenv.hostPlatform.system};
in
{
  options.dd.control = {
    enable = lib.mkEnableOption "the control plane's heartbeat and agreed state on this box";
    relay = lib.mkOption {
      type = lib.types.str;
      default = "wss://control.${config.dd.domain}/relay";
    };
    placement = lib.mkOption {
      type = lib.types.path;
      default = pkg.placement-inputs;
      defaultText = "the release's placement-inputs.json";
      description = "what the plan is made from: the release's placement inputs (a test may switch on its rules for joined boxes)";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [
      pkg.dd-control
      # dd-drain on: this box is given nothing new, and what may move moves
      # off it; what stays on failure stays until it leaves (control-plane.md)
      (pkgs.writeShellScriptBin "dd-drain" ''
        set -eu
        f=/var/lib/dd-facts/draining.prom
        [ "$(id -u)" = 0 ] || { echo "dd-drain: run it as root" >&2; exit 1; }
        case "''${1:-}" in
          on)
            install -d /var/lib/dd-facts
            echo "dd_box_draining 1" > "$f.new" && mv "$f.new" "$f"
            ;;
          off) rm -f "$f" ;;
          *) echo "usage: dd-drain on|off" >&2; exit 2 ;;
        esac
      '')
    ];
    environment.etc."dd-control/config.json".source = pkg.control-config;
    systemd.services.dd-control = {
      # the voters are in the config; a new list read only at start would sit
      # unread (176 did: no votes until a hand restart)
      restartTriggers = [
        pkg.control-config
        pkg.placement-inputs
        config.environment.etc."dd-placed/carries.json".source
        config.environment.etc."dd-placed/starts.json".source
      ];
      description = "The control plane: heartbeats and the agreed state";
      # a box that joined, still under the name it was installed with: it
      # joins once it has built itself under its key's (dd-agent), whose
      # switch starts this
      wantedBy = [ "multi-user.target" ];
      unitConfig.ConditionHost = lib.mkIf (config.networking.hostName == "joining") "!joining";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = [
        pkgs.tpm2-tools
        (import ../../lib/tpm-policy.nix pkgs)
      ]
      # where the network's control server runs: where each box that joined
      # on its own key is on the network, into the agreed state
      ++ lib.optional config.services.headscale.enable config.services.headscale.package;
      environment.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
      serviceConfig = {
        ExecStart = lib.concatStringsSep " " [
          "${pkg.dd-control}/bin/dd-control"
          "-config /etc/dd-control/config.json"
          "-box ${config.networking.hostName}"
          "-relay ${cfg.relay}"
          "-state /var/lib/dd-control"
          "-tpm-keys /boot/dd/unlock"
          "-listen 127.0.0.1:4195"
          # what the fleet would run, at /plan; switched here for the placed
          # services this box carries
          "-placement ${cfg.placement}"
          "-placed-carries /etc/dd-placed/carries.json"
          "-placed-starts /etc/dd-placed/starts.json"
          # which boxes' stores keep each bucket, for this box's copier
          (lib.optionalString (config.dd.store.enable or false) "-placed-copies /run/dd-placed/copies.json")
          # the gate hands in directory items here (servers/gate/src/directory.rs)
          (lib.optionalString config.dd.verify.enable "-submit /run/dd-control/submit.sock -submit-group dd-verify")
          # dd-attest hands in what it built for a build order here
          "-builds /run/dd-control/built.sock"
          # a box the release does not list joins on its own key: the one
          # its TPM holds, and its secrets key's recipient (dd-join-prepare)
          (lib.optionalString config.dd.box.joined "-join-key /var/lib/dd/box-key.spki -join-recipient-file /var/lib/dd/secrets-recipient -join-house joined -join-ticket-file /var/lib/dd/join-ticket")
          # dd-agent begins a release's rollout and gives its verdicts here
          "-rollouts /run/dd-control/rollout.sock"
        ];
        StateDirectory = "dd-control";
        StateDirectoryMode = "0700";
        RuntimeDirectory = "dd-control";
        RuntimeDirectoryMode = "0755";
        Restart = "always";
        RestartSec = 10;
        # root for the TPM, the key files on /boot and starting placed
        # services; nothing else is written
        ProtectSystem = "strict";
        ReadWritePaths = [
          "/var/lib/dd-control"
          # made by tmpfiles (box/placed.nix); a switch may start this first
          "-/run/dd-placed"
        ];
        ProtectHome = true;
        PrivateTmp = true;
        NoNewPrivileges = true;
      };
    };
  };
}
