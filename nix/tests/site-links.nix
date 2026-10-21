# The site's links, end to end: every link the pages show, after their
# scripts have run, followed in a browser against the gateway box's own
# front (node1's nginx config and the real gate with its pages, the
# programs behind them stood in: nix/tests/site/stack.py), each way a
# browser finds the box. And the app's own pages, read for where their
# links go. Every page it opens is also run through axe-core
# (site/a11y.py). No vm and no network. The report is $out/report.json.
{
  pkgs,
  self,
  lib,
  ...
}:
let
  system = pkgs.stdenv.hostPlatform.system;
  box = self.nixosConfigurations.node1.config;
  python = pkgs.python3.withPackages (ps: [
    ps.playwright
    ps.cryptography
  ]);
  # how nginx is started on the box, which names its config file
  start = toString box.systemd.services.nginx.serviceConfig.ExecStart;
  # axe-core for the accessibility pass, as site.nix fetches it
  axe = pkgs.fetchurl {
    url = "https://registry.npmjs.org/axe-core/-/axe-core-4.10.3.tgz";
    hash = "sha256-DytNfc330SGd+NGVmtaOVl9R0Uw/DYi7cc1Zq+uVYpI=";
  };
  env = pkgs.writeText "gate-env.json" (builtins.toJSON box.systemd.services.dd-verify.environment);
in
pkgs.runCommand "site-links"
  {
    nativeBuildInputs = [ python ];
    PLAYWRIGHT_BROWSERS_PATH = pkgs.playwright-driver.browsers;
    PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
    FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
    SITE_TESTER = ../../client/mail/tests/tester.json;
  }
  ''
    export HOME=$TMPDIR PYTHONPATH=${./site}:${./site-links}
    mkdir -p $TMPDIR/axe
    tar -xzf ${axe} -C $TMPDIR/axe package/axe.min.js
    export SITE_AXE=$TMPDIR/axe/package/axe.min.js
    conf=$(echo ${lib.escapeShellArg start} | grep -o '/nix/store/[^ ]*nginx.conf' | head -1)
    python3 ${./site-links/run.py} $out "$conf" ${box.services.nginx.package}/bin/nginx \
      ${self.packages.${system}.verify} ${env} \
      ${../../app/web} ${../../app/src} ${../../servers/gate/src}
  ''
