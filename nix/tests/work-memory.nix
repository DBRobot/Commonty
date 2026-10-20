# A memory hog in one CI job, on a box whose builds and CI share the work
# slice (modules/box/work-memory.nix): the kernel kills the hog inside the
# slice, the runner and its other job carry on, and a service outside the
# slice never notices. A unit left at systemd's default stops instead,
# which is what took node2's runner and every job on it down (2026-10-20).
{ pkgs, ... }:
let
  # one runner: a job that sleeps, and on /run/<unit>-hog a job that
  # allocates until it is killed
  worker =
    name:
    pkgs.writeShellScript "worker-${name}" ''
      sleep infinity &
      echo $! > /run/${name}-other
      while true; do
        if [ -e /run/${name}-hog ]; then
          rm /run/${name}-hog
          ${pkgs.python3}/bin/python3 -c '
      b = []
      while True:
          b.append(b"x" * (64 << 20))
      ' &
          echo $! > /run/${name}-hog-pid
        fi
        sleep 0.2
      done
    '';
  # memory something holds and keeps
  hold =
    mib:
    pkgs.writeShellScript "hold-${toString mib}" ''
      exec ${pkgs.python3}/bin/python3 -c 'import signal; x = b"x" * (${toString mib} << 20); signal.pause()'
    '';
in
{
  name = "work-memory";
  helpers = true;
  nodes.box = {
    imports = [
      ../modules/box/box.nix
      ../modules/box/work-memory.nix
    ];
    virtualisation.memorySize = 4096;
    # the boxes' 0: the test driver's 2 panics the vm on any kill, even
    # one inside a slice
    boot.kernel.sysctl."vm.panic_on_oom" = 0;
    # 4G: the slice gets half, 2G
    dd.box.memoryBytes = 4 * 1024 * 1024 * 1024;
    dd.work.units.runner.oomPolicy = "continue";
    dd.work.units.build = { };
    systemd.services.runner.serviceConfig.ExecStart = worker "runner";
    systemd.services.build.serviceConfig.ExecStart = worker "build";
    # a build order's build, holding its share meanwhile
    systemd.services.holder.serviceConfig.ExecStart = hold 400;
    dd.work.units.holder = { };
    # the box's own: outside the slice
    systemd.services.ours = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig.ExecStart = hold 300;
    };
  };
}
