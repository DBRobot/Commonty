# What CI keeps through a box's GC (.forgejo/checks/keep-inputs.sh) covers
# what a GC made the first run after it build again (run 768, 2026-10-12):
# the aarch64 cross rustc, crane's dependency builds, and the crates under
# clippy and the tests. Each job's keep step must name these installables,
# and their build graphs must hold those derivations.
# It reads .drv files, which pure evaluation forbids, so it is no flake
# check: CI's keep_check job runs `nix eval --impure --file nix/tests/ci-keep.nix`.
let
  self = builtins.getFlake (toString ../..);
  lib = self.inputs.nixpkgs.lib;
  c = self.checks.x86_64-linux;
  p = self.packages.x86_64-linux;
  # each keep step: its key and installables as the workflow names them,
  # the derivations they are, and what must be in their build graph
  keeps = [
    {
      g = "rust";
      key = "rust";
      items = [
        [
          ".#checks.x86_64-linux.fmt"
          c.fmt
        ]
        [
          ".#checks.x86_64-linux.clippy"
          c.clippy
        ]
        [
          ".#checks.x86_64-linux.tests"
          c.tests
        ]
      ];
      must = [
        "rust_dd-[0-9.]*\\.drv"
        "run-tests-rust_dd-[0-9.]*\\.drv"
      ];
    }
    {
      g = "app";
      key = "app_lint";
      items = [
        [
          ".#checks.x86_64-linux.clippy-app"
          c.clippy-app
        ]
        [
          ".#checks.x86_64-linux.tests-app"
          c.tests-app
        ]
        [
          ".#checks.x86_64-linux.net-check"
          c.net-check
        ]
      ];
      must = [ "commonty-deps-deps-[0-9.]*\\.drv" ];
    }
    {
      g = "cli";
      key = "part-\${{ matrix.part }}";
      items = [
        [
          ".#dd"
          p.dd
        ]
        [
          ".#web"
          p.web
        ]
      ];
      must = [ "dd-web-deps-deps-[0-9.]*\\.drv" ];
    }
    {
      # build_host keeps each host's; the home controller's holds the cross rustc
      g = "host";
      key = "host-\${{ matrix.host }}";
      items = [
        [
          ".#nixosConfigurations.\${{ matrix.host }}.config.system.build.toplevel"
          self.nixosConfigurations.home-controller.config.system.build.toplevel
        ]
      ];
      must = [ "aarch64-unknown-linux-gnu-rustc-[0-9.]*\\.drv" ];
    }
  ];
  # a step keeps what the job's build lines wrote down (--print-out-paths,
  # or build_host's tee of its toplevel)
  lines = lib.splitString "\n" workflow;
  recorded =
    i:
    builtins.any (
      l:
      lib.hasInfix i l
      && (lib.hasInfix "--print-out-paths" l || lib.hasInfix "tee \"$RUNNER_TEMP/toplevel\"" l)
    ) lines;
  # the derivations under some, by reading each .drv for the ones it names:
  # no build, and nothing has to be in the store but the .drv files
  drvsIn =
    d:
    builtins.filter builtins.isList (
      builtins.split "\"(/nix/store/[a-z0-9]{32}-[^\"]+\\.drv)\"" (builtins.readFile d)
    );
  graph =
    tops:
    map (x: x.key) (
      builtins.genericClosure {
        startSet = map (t: { key = builtins.unsafeDiscardStringContext t.drvPath; }) tops;
        operator = x: map (m: { key = builtins.head m; }) (drvsIn x.key);
      }
    );
  workflow = builtins.readFile ../../.forgejo/workflows/entry_point.yml;
  problems = lib.concatMap (
    k:
    let
      names = graph (map (x: lib.elemAt x 1) k.items);
    in
    lib.optional (
      !lib.hasInfix "keep-inputs.sh ${k.key} " workflow
    ) "the workflow does not run keep-inputs.sh ${k.key}"
    ++ map (i: "${k.key}: the job does not write down what it built from ${i}") (
      lib.filter (i: !recorded i) (map builtins.head k.items)
    )
    ++ map (pat: "${k.key} keeps no ${pat}") (
      lib.filter (
        pat: !builtins.any (n: builtins.match "/nix/store/[a-z0-9]{32}-${pat}" n != null) names
      ) k.must
    )
  ) keeps;
in
if problems != [ ] then
  throw ("\n" + lib.concatStringsSep "\n" problems)
else
  "the keep steps cover the cross rustc, crane's dependency builds and the crates"
