# The site as a browser meets it, without a vm: the gateway box's own nginx
# config and its gate, with the pages and the web bundle the release
# carries, and a stand-in for each service behind them (site/stack.py). One
# check each, run in seconds:
#
#   site-headers  HSTS, CSP, cookie flags and CORS on every name, and every
#                 gate route asked signed out (site/headers.py)
#   site-roles    who gets which page and API, a table per kind of person
#                 (site/roles.py)
#   site-a11y     axe-core on every page of the site map (site/a11y.py)
#   site-speed    click to drawn, and a first visit's bytes, against a
#                 budget (site/speed.py)
#
# The journeys are a vm of their own (site-journeys.nix): there the
# services are real.
{
  pkgs,
  self,
  lib,
  ...
}:
let
  system = pkgs.stdenv.hostPlatform.system;
  gateway = self.nixosConfigurations.node1.config;
  start = gateway.systemd.services.nginx.serviceConfig.ExecStart;
  nginx = "${gateway.services.nginx.package}/bin/nginx";
  env = pkgs.writeText "gate-env.json" (
    builtins.toJSON gateway.systemd.services.dd-verify.environment
  );
  python = pkgs.python3.withPackages (ps: [
    ps.playwright
    ps.cryptography
  ]);
  # axe-core, as npm publishes it; one file the page is given
  axe = pkgs.fetchurl {
    url = "https://registry.npmjs.org/axe-core/-/axe-core-4.10.3.tgz";
    hash = "sha256-DytNfc330SGd+NGVmtaOVl9R0Uw/DYi7cc1Zq+uVYpI=";
  };
  # the test's own files, and nothing else of the tree
  src = lib.fileset.toSource {
    root = ./site;
    fileset = lib.fileset.fileFilter (f: f.hasExt "py") ./site;
  };
  check =
    name: script: extra:
    pkgs.runCommand "site-${name}"
      {
        nativeBuildInputs = [ python ];
        PLAYWRIGHT_BROWSERS_PATH = pkgs.playwright-driver.browsers;
        PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
        SITE_TESTER = ../../client/mail/tests/tester.json;
      }
      ''
        export HOME=$TMPDIR
        mkdir -p $out run
        tar -xzf ${axe} -C run package/axe.min.js
        export SITE_AXE=$PWD/run/package/axe.min.js
        export PYTHONDONTWRITEBYTECODE=1
        # the config file, out of the unit's command line (as nginx.nix reads it)
        conf=$(echo ${lib.escapeShellArg (toString start)} | grep -o '/nix/store/[^ ]*nginx.conf' | head -1)
        cd ${src}
        status=0
        python3 ${script} $TMPDIR/run "$conf" ${nginx} ${self.packages.${system}.verify} ${env} ${extra} \
          > $out/report.txt 2>&1 || status=$?
        cat $out/report.txt
        if [ $status -ne 0 ]; then
          for f in gate nginx; do echo "--- $f"; tail -20 $TMPDIR/run/stack/$f.log 2>/dev/null; done
          exit $status
        fi
        cp $TMPDIR/run/speed.json $out/ 2>/dev/null || true
      '';
in
{
  site-headers = check "headers" "headers.py" "${../../servers/gate/src}";
  site-roles = check "roles" "roles.py" "";
  site-a11y = check "a11y" "a11y.py" "";
  site-speed = check "speed" "speed.py" "";
}
