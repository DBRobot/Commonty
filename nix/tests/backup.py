# The backup test: a backs up to its own store, the copy goes to b's, a
# dies, b restores a's files from its copy.
start_all()
for m in (a, b):
    m.wait_for_unit("dd-store-setup.service")
    m.wait_for_unit("nginx.service")
    m.wait_for_unit("timers.target")
    # every copy is one the test starts
    m.succeed(
        "systemctl stop dd-store-copy.timer dd-store-copy-recent.timer "
        "dd-store-copy.service dd-store-copy-recent.service"
    )

# each box's repository is a bucket of its own in its own store, under the
# store's own key to it
a.succeed("test -s /var/lib/dd-store/keys/backups-a.env")
a.succeed("systemctl cat restic-backups-dd | grep -c '127.0.0.1:3910/backups-a'")

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

# the copy: a's repository goes to b's store; b's copy of it never goes back
s3 = "AWS_DEFAULT_REGION=us-east-1 aws --endpoint-url http://127.0.0.1:3910"
# a copy that finds b not ready yet is next time: read back from b's store
# with the key b handed a
a.wait_until_succeeds(
    "systemctl start dd-store-copy.service && set -a && . /var/lib/dd-store-copy/b.env && "
    "AWS_DEFAULT_REGION=us-east-1 aws --endpoint-url http://store.b.test.invalid:3443 s3 ls s3://backups-a/config",
    timeout=180,
)
# what b holds of a's repository is b's word only: it never goes back to a
b.succeed(
    f"set -a; . /var/lib/dd-store/keys/backups-a.env; echo x > /tmp/x; {s3} s3 cp /tmp/x s3://backups-a/probe"
)
b.succeed("systemctl start dd-store-copy.service")
a.fail(
    f"set -a; . /var/lib/dd-store/keys/backups-a.env; {s3} s3 ls s3://backups-a/probe"
)
# b's own goes to a the same way
b.wait_until_succeeds(
    "systemctl start dd-store-copy.service && set -a && . /var/lib/dd-store-copy/a.env && "
    "AWS_DEFAULT_REGION=us-east-1 aws --endpoint-url http://store.a.test.invalid:3443 s3 ls s3://backups-b/config",
    timeout=180,
)

# b serves a's snapshots from its own copy before a goes
b_restic = (
    "set -a; . /var/lib/dd-store/keys/backups-a.env; "
    f"restic -r s3:http://127.0.0.1:3910/backups-a --password-file {nix['a_password']} --no-lock"
)
b.wait_until_succeeds(f"{b_restic} snapshots latest", timeout=180)

# a is gone. b's copy - a's password, b's own key to its copy - restores
# a's files on b
a.shutdown()
b.succeed(f"{b_restic} restore latest --target /tmp/restore")
b.succeed("grep -q 'worth having tomorrow' /tmp/restore/srv/state/file")
b.succeed("[ $(stat -c %s /tmp/restore/srv/state/blob) = 2000000 ]")

# and with a gone, b still backs itself up: its store is its own
b.succeed(
    "echo 'b, later' >> /srv/state/file && systemctl start restic-backups-dd.service"
)
