# Waiting in a VM test without a race: the patterns that made tests fail one
# run in five, done so they cannot. A test gets these with `helpers = true;`
# in its .nix (nix/lib/vm-test.nix puts this folder on the path), then
#
#     from waits import start_fresh, journal_of, journal_has, fetch_has
#
# Each takes the test driver's machine. docs/dev/ci.md says which pattern
# each one replaces and the runs that failed for it.
import shlex
import time


def _uptime_us(machine):
    return int(float(machine.succeed("cut -d' ' -f1 /proc/uptime").strip()) * 1e6)


def _show(machine, unit, prop):
    return machine.succeed(
        f"systemctl show -p {prop} --value {shlex.quote(unit)}"
    ).strip()


def start_fresh(machine, unit, timeout=300):
    """Run a oneshot unit once more and wait for that run to end: a run that
    was already under way when this was called (a timer's, a switch's) is
    waited out first and never taken for ours. Returns the run's invocation
    id, for journal_of. Fails if the run fails."""
    q = shlex.quote(unit)
    end = time.monotonic() + timeout
    # one already running began before what the test just set up: let it end
    while _show(machine, unit, "ActiveState") in (
        "activating",
        "deactivating",
        "reloading",
    ):
        if time.monotonic() > end:
            raise AssertionError(f"{unit}: a run under way did not end in {timeout}s")
        machine.sleep(1)
    since = _uptime_us(machine)
    while True:
        # start waits for a oneshot to finish; a run a timer began in the
        # instant between is joined, so the start time below decides
        machine.succeed(f"systemctl start {q}")
        began = int(_show(machine, unit, "ExecMainStartTimestampMonotonic") or 0)
        if began >= since:
            break
        if time.monotonic() > end:
            raise AssertionError(f"{unit}: no run of its own within {timeout}s")
    inv = _show(machine, unit, "InvocationID")
    result = _show(machine, unit, "Result")
    if result != "success":
        raise AssertionError(
            f"{unit}: its run {inv} ended {result}:\n{journal_of(machine, unit, inv)}"
        )
    return inv


def journal_of(machine, unit, invocation):
    """The lines of one run of a unit, and no other run's."""
    return machine.succeed(
        f"journalctl --no-pager -o cat _SYSTEMD_INVOCATION_ID={shlex.quote(invocation)}"
    )


def journal_has(machine, unit, invocation, text):
    """Whether one run of a unit wrote `text` (a plain string, not a pattern)."""
    return text in journal_of(machine, unit, invocation)


def wait_unit_active(machine, unit, timeout=300):
    """A long-running unit up: active, not still starting."""
    machine.wait_until_succeeds(
        f"systemctl is-active -q {shlex.quote(unit)}", timeout=timeout
    )


def wait_socket(machine, where, timeout=300):
    """Something listening: a TCP port (an int) or a unix socket (a path)."""
    if isinstance(where, int):
        machine.wait_for_open_port(where, timeout=timeout)
    else:
        machine.wait_until_succeeds(f"test -S {shlex.quote(where)}", timeout=timeout)


def fetch_has(machine, url, text, curl_args="-sf"):
    """Whether `url` answers with a body holding `text`. The body goes to a
    file first: `curl | grep -q` lets grep leave at the first match, curl
    then dies of SIGPIPE writing the rest, and the pipe fails on a page
    that had the text (exit code 23)."""
    out = machine.succeed("mktemp").strip()
    code, _ = machine.execute(
        f"curl {curl_args} -o {out} {shlex.quote(url)} && grep -cF -- {shlex.quote(text)} {out}; r=$?; rm -f {out}; exit $r"
    )
    return code == 0


def wait_fetch_has(machine, url, text, timeout=300, curl_args="-sf"):
    """Wait until `url` answers with a body holding `text`."""
    end = time.monotonic() + timeout
    while not fetch_has(machine, url, text, curl_args):
        if time.monotonic() > end:
            raise AssertionError(f"{url} never held {text!r} in {timeout}s")
        machine.sleep(1)
