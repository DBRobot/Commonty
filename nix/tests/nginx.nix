# Every box's nginx config, through `nginx -t`, before a box ever sees it.
# Building a config checks nothing of its syntax: release 115 went out with
# a location regex nginx could not read, and node1 rolled back. The
# certificates and the files a box writes at run time are not here, so this
# points the one at a throwaway certificate and the others at empty files.
{
  pkgs,
  self,
  lib,
  ...
}:
let
  boxes = lib.filterAttrs (_: c: c.config.services.nginx.enable) self.nixosConfigurations;
  one =
    name: c:
    let
      start = c.config.systemd.services.nginx.serviceConfig.ExecStart;
      nginx = "${c.config.services.nginx.package}/bin/nginx";
    in
    ''
      echo "== ${name}"
      conf=$(echo ${lib.escapeShellArg (toString start)} | grep -o '/nix/store/[^ ]*nginx.conf' | head -1)
      python3 ${./nginx-t.py} "$conf" ${nginx} "$tmp"
    '';
in
pkgs.runCommand "nginx-configs-parse"
  {
    nativeBuildInputs = [
      pkgs.openssl
      pkgs.python3
    ];
  }
  ''
    tmp=$(mktemp -d)
    openssl req -x509 -newkey rsa:2048 -nodes -keyout $tmp/key.pem -out $tmp/cert.pem -days 1 -subj /CN=t 2>/dev/null
    ${lib.concatStrings (lib.mapAttrsToList one boxes)}
    touch $out
  ''
