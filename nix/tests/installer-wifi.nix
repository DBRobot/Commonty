# The box installer's Wi-Fi step (dd-box-setup /api/wifi) against a real
# access point: two simulated radios (mac80211_hwsim), one a WPA2 network
# from hostapd, the other the installer's under NetworkManager. A wrong
# password is refused with NetworkManager's reason; the right one joins, as
# typed and as the drive carries it.
{
  pkgs,
  lib,
  self,
  ...
}:
{
  name = "installer-wifi";
  node.specialArgs = { inherit self; };
  nodes.blank =
    { ... }:
    {
      imports = [ ../installer/setup.nix ];
      boot.kernelModules = [ "mac80211_hwsim" ];
      boot.extraModprobeConfig = "options mac80211_hwsim radios=2";
      # the access point's radio is hostapd's alone
      networking.networkmanager.unmanaged = [ "wlan0" ];
      systemd.services.dd-box-setup.environment.SETUP_DRIVE = "/root/drive";
      environment.systemPackages = [
        pkgs.hostapd
        pkgs.curl
        pkgs.jq
      ];
      environment.variables = lib.mkForce {
        RELEASE_URL = "http://unused/release/current.json";
        RELEASE_KEY = "/unused";
        SOURCE = "git+http://unused/flake.git";
        DETECT = "/unused";
        BOX_JSON = "${../installer/box.json}";
        NOT_BEFORE = "100";
        FILES = "http://unused/files";
      };
      environment.etc."hostapd.conf".text = ''
        interface=wlan0
        driver=nl80211
        ssid=House Wi-Fi
        hw_mode=g
        channel=6
        wpa=2
        wpa_key_mgmt=WPA-PSK
        rsn_pairwise=CCMP
        wpa_passphrase=correct horse battery
      '';
      virtualisation.memorySize = 2048;
    };
}
