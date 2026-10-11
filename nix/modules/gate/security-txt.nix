# /.well-known/security.txt on the bare domain (RFC 9116): where to report
# a hole. Expires is a fixed date: one taken from the commit made a merge
# commit's system differ from the one CI built. CI fails 60 days before it.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  domain = config.dd.domain;
  url = "https://${domain}/.well-known/security.txt";
  file = pkgs.runCommand "security.txt" { } ''
    cat > $out <<EOF
    Contact: mailto:security@${domain}
    Expires: ${config.dd.securityTxtExpires}
    Preferred-Languages: en
    Canonical: ${url}
    Policy: https://${domain}/docs/policies/privacy
    EOF
    sed -i 's/^ *//' $out
  '';
in
{
  options.dd.securityTxtExpires = lib.mkOption {
    type = lib.types.strMatching "[0-9]{4}-[0-9]{2}-[0-9]{2}T00:00:00Z";
    default = "2027-10-01T00:00:00Z";
    description = "When security.txt stops vouching for itself; moved on by hand, at most a year out (CI warns 60 days before).";
  };
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
