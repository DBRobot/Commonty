from waits import (
    fetch_has,
    journal_has,
    journal_of,
    start_fresh,
    wait_fetch_has,
    wait_socket,
    wait_unit_active,
)

box.wait_for_unit("multi-user.target")

# start_fresh: every call is a run of its own, begun after it was called,
# even with a timer starting runs every three seconds; its journal is that
# run's lines alone
seen = set()
for _ in range(8):
    before = int(box.succeed("cat /var/lib/slow-runs 2>/dev/null || echo 0"))
    inv = start_fresh(box, "slow.service")
    lines = journal_of(box, "slow.service", inv)
    began = [ln for ln in lines.splitlines() if ln.endswith(" began")]
    assert len(began) == 1, f"one run's journal holds {began}"
    n = int(began[0].split()[1])
    assert n > before, f"run {n} began before the call (runs were at {before})"
    assert journal_has(box, "slow.service", inv, f"run {n} done")
    assert not journal_has(box, "slow.service", inv, f"run {n - 1} done")
    assert inv not in seen
    seen.add(inv)

# journal_of: a run's last line, which journald reads only once the run has
# ended (its writer gone, the unit inactive: no invocation id is left to
# give the line), is still that run's. pg-follow failed on this (CI run 767)
box.succeed("systemctl start --no-block last-word")
box.wait_until_succeeds('test -n "$(systemctl show -p InvocationID --value last-word)"')
inv = box.succeed("systemctl show -p InvocationID --value last-word").strip()
box.wait_until_succeeds(
    f"journalctl -o cat _SYSTEMD_INVOCATION_ID={inv} > /tmp/lw && grep -c 'first word' /tmp/lw"
)
journald = box.succeed("systemctl show -p MainPID --value systemd-journald").strip()
box.succeed(f"kill -STOP {journald}")
box.succeed("touch /run/last-word-go")
box.wait_until_succeeds(
    'test "$(systemctl show -p ActiveState --value last-word)" = inactive'
)
box.succeed(f"kill -CONT {journald}")
box.wait_until_succeeds(
    "journalctl -o cat > /tmp/all && grep -c 'the last word' /tmp/all"
)
assert journal_has(box, "last-word.service", inv, "the last word"), journal_of(
    box, "last-word.service", inv
)

# fetch_has: a page with the word, however much comes after it; a word it
# does not hold is not found
wait_socket(box, 8088)
for _ in range(20):
    assert fetch_has(box, "http://127.0.0.1:8088/", "hello-there")
assert not fetch_has(box, "http://127.0.0.1:8088/", "not-on-the-page")
wait_fetch_has(box, "http://127.0.0.1:8088/", "hello-there", timeout=30)

# a socket that comes up late is waited for, then answers
wait_socket(box, "/run/late.sock", timeout=60)
wait_unit_active(box, "late-socket.service")
assert box.succeed("echo ping | socat - UNIX-CONNECT:/run/late.sock").strip() == "ping"
