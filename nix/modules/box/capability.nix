# What this box can run, judged on the box: the release's own rules
# (nix/lib/capability.nix) on its report as it is now (box/report.nix, made
# at every boot), with its owner's box.json, the needs and the policy this
# release ships. Written to /var/lib/dd-facts/can.prom; dd-control carries
# it in the heartbeat and placement uses it over the release's table, so a
# box that gains a GPU or memory is placed for it the next round, with no
# release. A box's word never adds what its box.json or the policy bar, nor
# a lease the release does not let it hold (servers/system/control/place).
# On no report or a failed judgment the file goes, and the release's table
# stands.
dir:
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  pkg = self.packages.${pkgs.stdenv.hostPlatform.system};
  host = config.networking.hostName;
  # a joined box's owner is the agreed state's (lib/joined-own.nix)
  own =
    if config.dd.box.joined then
      pkgs.writeText "box.json" (builtins.toJSON (import ../../lib/joined-own.nix { inherit lib; } dir))
    else
      dir + "/box.json";
  judge = pkgs.writeText "capability-judge.nix" ''
    { hw }:
    let
      lib = import ${
        builtins.path {
          path = pkgs.path + "/lib";
          name = "nixpkgs-lib";
        }
      };
    in
    (import ${../../lib/capability.nix} {
      inherit lib;
      needs = lib.importJSON ${pkg.needs};
      policy = lib.importJSON ${../../../fleet/policy.json};
      boxes.${host} = {
        hw = lib.importJSON hw;
        own = lib.importJSON ${own};
      };
    }).boxes.${host}
  '';
in
{
  systemd.services.dd-capability = {
    description = "What this box can run, by the release's rules on its report as it is now";
    wantedBy = [ "multi-user.target" ];
    after = [ "dd-box-report.service" ];
    wants = [ "dd-box-report.service" ];
    path = [
      config.nix.package
      pkgs.jq
      pkgs.coreutils
    ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
    };
    # run again by every switch that changes the needs or the rules (the
    # judge's path is in this script); never fails it
    script = ''
      set -u
      out=/var/lib/dd-facts/can.prom
      hw=/var/lib/dd/report/hardware.json
      if [ ! -s "$hw" ] || ! j=$(nix-instantiate --eval --strict --json --readonly-mode --argstr hw "$hw" ${judge}); then
        rm -f "$out"
        echo "no live judgment; the release's table stands"
        exit 0
      fi
      install -d /var/lib/dd-facts
      { echo "dd_can_live 1"; jq -r '.can | to_entries[] | "dd_can{service=\"\(.key)\"} \(.value.score)"' <<<"$j"; } > "$out.new" && mv "$out.new" "$out"
      echo "can run: $(jq -r '.can | keys | join(" ")' <<<"$j")"
      exit 0
    '';
  };
}
