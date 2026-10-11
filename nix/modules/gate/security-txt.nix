# /.well-known/security.txt on the bare domain (RFC 9116): where to report
# a hole. Expires is a year after the commit the release is built from, so
# a fleet that stops releasing lets it lapse rather than vouch for it.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  domain = config.dd.domain;
  url = "https://${domain}/.well-known/security.txt";
  file =
    pkgs.runCommand "security.txt"
      {
        nativeBuildInputs = [ pkgs.coreutils ];
      }
      ''
        expires=$(date -u -d @$(( ${toString self.lastModified} + 365 * 24 * 3600 )) +%Y-%m-%dT%H:%M:%SZ)
        cat > $out <<EOF
        Contact: mailto:security@${domain}
        Expires: $expires
        Preferred-Languages: en
        Canonical: ${url}
        Policy: https://${domain}/docs/policies/privacy
        EOF
        sed -i 's/^ *//' $out
      '';
in
{
  options.dd.securityTxt = lib.mkOption {
    type = lib.types.package;
    readOnly = true;
    default = file;
    description = "The bare domain's security.txt, as built (nix/tests/nginx.nix checks it).";
  };

  config.services.nginx.virtualHosts.${domain}.locations."= /.well-known/security.txt" = {
    alias = file;
    extraConfig = ''
      default_type "text/plain; charset=utf-8";
      limit_req zone=dd_visitor burst=20 nodelay;
    '';
  };
}
