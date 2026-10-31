import time

from waits import wait_socket, wait_unit_active

seconds = nix["seconds"]


def boot(mode):
    """up, and the drain watching, with mode's stand-in for dd-control and
    this box's service running (no mode: no control plane; "none": no service)"""
    box.wait_for_unit("multi-user.target")
    wait_unit_active(box, "dd-shutdown-drain.service")
    box.wait_until_succeeds(
        "journalctl -b -u dd-shutdown-drain -o cat | grep -c 'watching for a shutdown'",
        timeout=60,
    )
    box.succeed("rm -f /var/lib/standin/log /var/lib/standin/mode")
    assert box.execute("test -e /var/lib/dd-facts/draining.prom")[0] != 0, (
        "the last shutdown's drain outlived it"
    )
    if mode != "none":
        box.succeed("touch /run/dd-placed/svc")
    if mode not in ("none", "down"):
        box.succeed(f"echo {mode} > /var/lib/standin/mode && systemctl restart standin")
        wait_socket(box, 4195)
    else:
        box.succeed("systemctl stop standin")


def button():
    """the power button; the seconds until the box is off"""
    start = time.monotonic()
    box.send_monitor_command("system_powerdown")
    box.wait_for_shutdown()
    took = time.monotonic() - start
    box.start()
    return took


def said():
    return box.succeed("journalctl -b -1 -u dd-shutdown-drain -o cat --no-pager")


def log():
    return box.succeed("cat /var/lib/standin/log").split()


box.start()
# logind ends the wait itself, whatever the drain does
boot("none")
assert box.succeed(
    "busctl get-property org.freedesktop.login1 /org/freedesktop/login1"
    " org.freedesktop.login1.Manager InhibitDelayMaxUSec"
).split()[1] == str((seconds + 15) * 1000000)
# nothing served: nothing to wait for
button()
assert "nothing here to hand on" in said()

# what it runs moves, while it still runs here
boot("moves")
button()
assert "handed on in" in said(), said()
assert log() == ["moved", "stopped"], log()

# no control plane: nothing can be handed on, so it does not wait
boot("down")
button()
assert "the control plane does not answer" in said(), said()

# the plan keeps it here (nowhere else can run it): it does not wait the bound
boot("stays")
button()
assert "the rest stays with this box" in said(), said()

# it never moves: the bound
boot("hangs")
button()
assert "gave up after" in said(), said()
assert log() == ["stopped"], log()

# and a drain that wedges holds the shutdown no longer than logind's limit
boot("hangs")
box.succeed("systemctl kill --signal=SIGSTOP dd-shutdown-drain")
took = button()
assert took < seconds + 15 + 60, f"the power button took {took:.0f}s"
boot("none")
