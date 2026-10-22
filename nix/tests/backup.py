# The backup test: a backs up to the cluster, a dies, b restores a's files.

start_all()
for m in (a, b):
    m.wait_for_unit("garage.service")
    m.wait_for_open_port(3901)

# the boxes find each other the way the real ones do: b is told a's id
# (in the fleet it comes from the box list), then each stages its own
# role and the layout applies once both are in
a_id = a.succeed(f"set -a; . {nix['rpc']}; garage node id -q").strip().split("@")[0]
b.succeed(f"set -a; . {nix['rpc']}; garage node connect {a_id}@a:3901")
for m in (a, b):
    m.succeed("systemctl restart garage-setup.service")
# both boxes hold the applied layout before the buckets and keys are made
for m in (a, b):
    m.wait_until_succeeds(
        f"set -a; . {nix['rpc']}; garage layout show > /tmp/l && grep -q 'Current cluster layout version: [1-9]' /tmp/l"
    )
for m in (a, b):
    m.succeed(
        "systemctl restart garage-setup.service"
    )  # buckets and keys, now that the layout exists

# a backs up, b backs up
a.succeed(
    "mkdir -p /srv/state && echo 'the thing worth having tomorrow' > /srv/state/file && head -c 2000000 /dev/urandom > /srv/state/blob"
)
b.succeed("mkdir -p /srv/state && echo 'b has state too' > /srv/state/file")
a.succeed("systemctl start restic-backups-dd.service")
b.succeed("systemctl start restic-backups-dd.service")
a.succeed("grep -q dd_backup_last_success_seconds /var/lib/dd-facts/backup.prom")

# a backup gone stale heals itself before it mails anyone: its nightly timer
# stopped (as node2's lost its next run, 2026-10-12) and the last good backup
# a week old, the check starts the timer again and backs up now
a.succeed("systemctl stop restic-backups-dd.timer")
a.succeed(
    "sed -i 's/^\\(dd_backup_last_success_seconds{[^}]*}\\) .*/\\1 1/' /var/lib/dd-facts/backup.prom"
)
a.succeed("systemctl start dd-backup-stale.service")
a.succeed("systemctl is-active restic-backups-dd.timer")
a.succeed(
    "[ $(awk '/^dd_backup_last_success_seconds/ {print $2}' /var/lib/dd-facts/backup.prom) -gt $(( $(date +%s) - 3600 )) ]"
)
# and the timer watch runs clean, with a count the fleet view reads
a.succeed("systemctl start dd-timer-watch.service")
a.succeed("grep -q '^dd_timers_restarted 0' /var/lib/dd-facts/timers.prom")

# every block is on both boxes
a.wait_until_succeeds("[ $(find /srv/garage -type f | wc -l) -ge 3 ]")
b.wait_until_succeeds("[ $(find /srv/garage -type f | wc -l) -ge 3 ]")

# blocks are not the bucket: b reads bucket names from its own tables, and
# a write that missed b (one copy of two is enough) reaches it later. Wait
# until b alone resolves backups-a and serves a's snapshots
b_restic = (
    f"set -a; . {nix['a_env']}; "
    f"restic -r s3:http://127.0.0.1:3900/backups-a --password-file {nix['a_password']} --no-lock"
)


def b_view():
    return b.execute(
        f"set -a; . {nix['rpc']}; garage bucket list; garage bucket info backups-a 2>&1; garage key list"
    )[1]


b.wait_until_succeeds(f"set -a; . {nix['rpc']}; garage bucket info backups-a")
b.wait_until_succeeds(f"{b_restic} snapshots latest")
view_before = b_view()

# a is gone. b still answers, and a's repository - a's password, a's key -
# restores a's files on b. Without the lock: a lock is a write, and the
# cluster takes no writes with one box down
a.shutdown()
try:
    b.succeed(f"{b_restic} restore latest --target /tmp/restore")
except Exception:
    print(f"b before a went:\n{view_before}\nb now:\n{b_view()}")
    raise
b.succeed("grep -q 'worth having tomorrow' /tmp/restore/srv/state/file")
b.succeed("[ $(stat -c %s /tmp/restore/srv/state/blob) = 2000000 ]")

# with a down the cluster refuses new writes: two copies or nothing.
# Asked directly, not through restic, which retries for a quarter hour
# before it agrees
# b learns a is gone a moment after it is; until then a write may still
# land on a as it dies. Wait for the refusal rather than expect it at once
b.wait_until_fails(
    f"set -a; . {nix['b_env']}; "
    "AWS_MAX_ATTEMPTS=1 aws --endpoint-url http://127.0.0.1:3900 s3 cp /etc/hostname s3://backups-b/probe"
)
