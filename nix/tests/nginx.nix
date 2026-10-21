# Every box's nginx config, through `nginx -t`, before a box ever sees it;
# and the forge's public door, served and asked (nginx-door.py), and the
# bare name's front page (nginx-front.py).
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
      vault = c.config.services.nginx.virtualHosts ? "vault.${c.config.dd.domain}";
    in
    ''
      echo "== ${name}"
      conf=$(echo ${lib.escapeShellArg (toString start)} | grep -o '/nix/store/[^ ]*nginx.conf' | head -1)
      python3 ${./nginx-t.py} "$conf" ${nginx} "$tmp"
      # the fleet's metrics never answer outsiders (roles/observe.nix)
      if grep -q 'demo-metrics\.' "$conf"; then echo "${name}: a demo door to Metrics"; exit 1; fi
    ''
    # the forge's public door, run: each address it lets through reaches
    # the right place as nobody, and every other is refused before it does
    + lib.optionalString (c.config.services.nginx.virtualHosts ? "code.${c.config.dd.domain}") ''
      python3 ${./nginx-door.py} "$conf" ${nginx} "$tmp" \
        code.${c.config.dd.domain} git.${c.config.dd.domain} ${c.config.dd.repo}
    ''
    # the bare name's front page served at /, home's / still signing in, and
    # the docs and the download as they were; Passwords' public name hands
    # a browser to the app, and a Send still opens
    +
      lib.optionalString
        (c.config.dd ? public && c.config.dd.public.enable && (c.config.dd.public.bare || vault))
        ''
          python3 ${./nginx-front.py} "$conf" ${nginx} "$tmp" ${c.config.dd.domain} ${
            lib.concatStringsSep " " (
              lib.optional c.config.dd.public.bare "front" ++ lib.optional vault "vault"
            )
          }
        ''
    # the bare domain's security.txt: served, and saying what RFC 9116 asks
    + lib.optionalString (c.config.dd ? securityTxt) ''
      grep -q 'location = /.well-known/security.txt' "$conf"
      python3 ${./security-txt.py} ${c.config.dd.securityTxt} ${c.config.dd.domain}
    '';
in
pkgs.runCommand "nginx-configs-parse"
  {
    nativeBuildInputs = [
      pkgs.openssl
      pkgs.python3
      pkgs.curl
    ];
  }
  ''
    tmp=$(mktemp -d)
    openssl req -x509 -newkey rsa:2048 -nodes -keyout $tmp/key.pem -out $tmp/cert.pem -days 1 -subj /CN=t 2>/dev/null
    ${lib.concatStrings (lib.mapAttrsToList one boxes)}
    touch $out
  ''
