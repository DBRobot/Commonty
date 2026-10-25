import base64
import json
import shlex

from waits import (
    journal_cursor,
    journal_of,
    journal_since,
    start_fresh,
    wait_unit_active,
)

runner = nix["runner"]
members = nix["members"]
owner = nix["owner"]
as_owner = (
    f"curl -sf --unix-socket {nix['sock']} -H 'X-WEBAUTH-USER: {owner}' "
    "-H 'content-type: application/json'"
)
api = f"http://forgejo/api/v1/repos/{owner}/commonty"


def show(prop, unit=runner):
    return box.succeed(f"systemctl show -P {prop} {shlex.quote(unit)}").strip()


def config_capacity(unit=runner):
    """the capacity in the config the running runner was started with"""
    # its main process is runner-start.sh until the token is fetched and it
    # execs the runner: active is not yet the runner's own config
    conf = box.wait_until_succeeds(
        "tr '\\0' '\\n' < /proc/$(systemctl show -P MainPID "
        f"{shlex.quote(unit)})/cmdline | grep -x -A1 -- --config | sed -n 2p | grep .",
        timeout=120,
    ).strip()
    return box.succeed(f"grep -o 'capacity: [0-9]*' {conf}").split()[1]


def run_of(number):
    runs = json.loads(box.succeed(f"{as_owner} '{api}/actions/runs?limit=20'"))
    return next(r for r in runs["workflow_runs"] if r["index_in_repo"] == number)


def wait_run(number, *status):
    want = " or ".join(f'.status == "{s}"' for s in status)
    box.wait_until_succeeds(
        f"{as_owner} '{api}/actions/runs?limit=20' | jq -e "
        f"'.workflow_runs[] | select(.index_in_repo == {number}) | {want}'",
        timeout=300,
    )


def restarted(unit, before, timeout):
    box.wait_until_succeeds(
        f'[ "$(systemctl show -P InvocationID {shlex.quote(unit)})" != "{before}" ]',
        timeout=timeout,
    )
    wait_unit_active(box, unit)


# the system the releases below are specialisations of
base = box.succeed("readlink -f /run/current-system").strip()

box.wait_for_unit("forgejo.service")
box.wait_until_succeeds(f"test -S {nix['sock']}", timeout=120)
box.wait_until_succeeds(
    '[ -n "$(systemctl show -P ExecMainExitTimestamp forgejo-admin.service)" ]',
    timeout=180,
)
box.wait_for_unit("nginx.service")
box.succeed(
    f'{as_owner} -X POST -d \'{{"name":"commonty","default_branch":"main","auto_init":true}}\' '
    "http://forgejo/api/v1/user/repos >/dev/null"
)

# the runners register with tokens the forge's box makes and seals to this
# one, as on a box: nobody mints one by hand
box.succeed("systemctl start dd-runner-keys.service")
box.succeed("test -s /var/lib/dd-runner-keys/box.host.age")
for unit in (runner, members):
    u = shlex.quote(unit)
    box.succeed(f"systemctl reset-failed {u} || true; systemctl restart {u}")
    wait_unit_active(box, unit)
    box.wait_until_succeeds(
        f"journalctl -u {u} > /tmp/runner.log; grep -c 'declared successfully' /tmp/runner.log"
    )
    assert config_capacity(unit) == "1"
box.succeed("grep -q '^TOKEN=.' /var/lib/dd-runner-token/host.env")
box.succeed("grep -q '^TOKEN=.' /var/lib/dd-runner-token/members.env")
# what a member's job runs in, there before the job asks for it
box.succeed(f"docker load < {nix['jobImage']}")
# the runners and docker on their first config are current: the restart
# unit leaves them be
docker_before = show("InvocationID", "docker.service")
start_fresh(box, "dd-runner-restart.service")
assert show("ActiveState") == "active"
assert show("ActiveState", members) == "active"
assert show("InvocationID", "docker.service") == docker_before

# a job that runs until the test lets it end
job = (
    "on: push\n"
    "jobs:\n"
    "  long:\n"
    "    runs-on: nix\n"
    "    steps:\n"
    "      - run: |\n"
    "          touch /run/job-gate/started-${{ github.run_number }}\n"
    "          while [ ! -e /run/job-gate/go-${{ github.run_number }} ]; do sleep 1; done\n"
    "          echo the job ran to its end\n"
)


# a member's job: in its container, under gVisor, with nothing of the box
# mounted, so the test sees it start and lets it end through docker
member_job = (
    "on: push\n"
    "jobs:\n"
    "  member:\n"
    "    runs-on: members\n"
    "    steps:\n"
    "      - shell: sh\n"
    "        run: |\n"
    "          touch /tmp/started\n"
    "          while [ ! -e /tmp/go ]; do sleep 1; done\n"
    "          echo the member job ran to its end\n"
)
# the member's job's container: the one with the members' runner's label
jobs = "docker ps -q --filter label=dd.runner=members-box"
in_job = f"docker exec $({jobs}) "


# main is protected (forgejo-protection): each job is a push to a branch
def push(name, job=job):
    content = base64.b64encode(job.encode()).decode()
    box.succeed(
        f'{as_owner} -X POST -d \'{{"branch":"main","new_branch":"{name}","content":"{content}"}}\' '
        f"{api}/contents/.forgejo/workflows/{name}.yml >/dev/null"
    )


push("long")
box.wait_until_succeeds("test -e /run/job-gate/started-1", timeout=300)
before = show("InvocationID")
push("member", member_job)
box.wait_until_succeeds(f"{in_job} test -e /tmp/started", timeout=300)
members_before = show("InvocationID", members)
container = box.succeed(jobs).strip()

# the release: the runners' units change, docker's config changes, the
# gate stops and starts
box.succeed(
    "/run/current-system/specialisation/next/bin/switch-to-configuration test >&2"
)
assert show("InvocationID") == before, "the switch restarted the runner"
assert show("ActiveState") == "active"
assert config_capacity() == "1"
assert show("InvocationID", "docker.service") == docker_before, (
    "the switch restarted docker"
)
assert show("InvocationID", members) == members_before, (
    "the switch restarted the members' runner"
)
assert config_capacity(members) == "1"
assert box.succeed(jobs).strip() == container, "the job's container went"
assert "release=next" not in box.succeed("docker info --format '{{.Labels}}'")
# it is the new unit on disk, and the restart unit waits for the job. Its
# main process is runner-start.sh, whose CONFIG is the runner's config (a
# step at a time: an empty path to cat read the test's stdin and hung it)
start = box.succeed(
    f"systemctl show -P ExecStart {runner} | grep -o 'path=[^ ;]*' | cut -d= -f2"
).strip()
conf = box.succeed(f"sed -n 's/^export CONFIG=//p' {start} | tr -d \"'\"").strip()
assert conf.startswith("/nix/store/"), (start, conf)
assert "capacity: 2" in box.succeed(f"cat {conf} </dev/null")
inv = start_fresh(box, "dd-runner-restart.service")
said = journal_of(box, "dd-runner-restart.service", inv)
for unit in (runner, members, "docker.service"):
    assert f"{unit} changed; a job is running" in said, said
assert show("InvocationID") == before
assert show("InvocationID", members) == members_before
assert show("InvocationID", "docker.service") == docker_before
assert run_of(1)["status"] == "running", run_of(1)
assert run_of(2)["status"] == "running", run_of(2)

# the member's job ends, and the forge hears it passed
box.succeed(f"{in_job} touch /tmp/go")
wait_run(2, "success")
# with no container left, docker and the members' runner restart into
# their new config
box.wait_until_succeeds(
    f'[ "$(systemctl show -P InvocationID docker.service)" != "{docker_before}" ]',
    timeout=240,
)
box.wait_until_succeeds(
    f'[ "$(systemctl show -P InvocationID {shlex.quote(members)})" != "{members_before}" ]',
    timeout=240,
)
wait_unit_active(box, "docker.service")
wait_unit_active(box, members)
assert config_capacity(members) == "2"
assert "release=next" in box.succeed("docker info --format '{{.Labels}}'")

# the job ends while the gate is down: the runner still says so to the forge
box.succeed("systemctl stop dd-verify.service")
box.succeed("touch /run/job-gate/go-1")
wait_run(1, "success")
box.succeed("systemctl start dd-verify.service")

# with no job left, the timer restarts the runner into its new config
box.wait_until_succeeds(
    f'[ "$(systemctl show -P InvocationID {runner})" != "{before}" ]', timeout=240
)
wait_unit_active(box, runner)
assert config_capacity() == "2"
inv = start_fresh(box, "dd-runner-restart.service")
assert "restarting" not in journal_of(box, "dd-runner-restart.service", inv)

# a stop that meets a job anyway (a restart in the instant a job is taken)
# lets it finish, then restarts
push("again")
box.wait_until_succeeds("test -e /run/job-gate/started-3", timeout=300)
mid = show("InvocationID")
box.succeed(f"systemctl restart --no-block {runner}")
box.wait_until_succeeds(
    f'[ "$(systemctl show -P ActiveState {runner})" = deactivating ]', timeout=30
)
box.succeed("touch /run/job-gate/go-3")
wait_run(3, "success")
box.wait_until_succeeds(
    f'[ "$(systemctl show -P InvocationID {runner})" != "{mid}" ]', timeout=120
)
wait_unit_active(box, runner)
# and the job was not cancelled on the way: it is the forge's success above,
# and the runner never forced one
box.succeed(f"journalctl -u {runner} > /tmp/runner.log")
assert (
    box.succeed(
        "grep -c 'forcing the jobs to shutdown' /tmp/runner.log || true"
    ).strip()
    == "0"
), "the runner forced a job to stop"

# A member's job that never ends does not hold docker's new config back for
# good: past the limit (4 min here) the members' runner drains (1 min here,
# then the job is cancelled) and docker restarts. A container on the box
# that is no member's job counts for nothing.
box.succeed(
    "docker run -d --name other ghcr.io/catthehacker/ubuntu:act-22.04 sleep 100000"
)
docker_before = show("InvocationID", "docker.service")
members_before = show("InvocationID", members)
push("member-long", member_job)
box.wait_until_succeeds(f"{in_job} test -e /tmp/started", timeout=300)
box.succeed(f"{base}/specialisation/later/bin/switch-to-configuration test >&2")
assert show("InvocationID", "docker.service") == docker_before, (
    "the switch restarted docker"
)
inv = start_fresh(box, "dd-runner-restart.service")
said = journal_of(box, "dd-runner-restart.service", inv)
assert "docker.service changed; a job is running (1 docker)" in said, said
box.succeed(
    "grep -E '^dd_runner_restart_pending_seconds\\{unit=\"docker.service\"\\} [0-9]+$' "
    "/var/lib/dd-facts/runner-restart.prom"
)
restarted("docker.service", docker_before, timeout=900)
said = box.succeed("journalctl -u dd-runner-restart.service")
assert "docker.service: changed" in said and "past its limit (240s)" in said, said
wait_run(4, "failure", "cancelled")
assert "release=later" in box.succeed("docker info --format '{{.Labels}}'")
restarted(members, members_before, timeout=240)
box.wait_until_succeeds(
    "grep -qx 'dd_runner_restart_pending_seconds{unit=\"docker.service\"} 0' "
    "/var/lib/dd-facts/runner-restart.prom",
    timeout=180,
)

# A change to the sandbox (here gVisor's flags) does not wait for the job at
# all: the next pass stops the members' runner, the job on the old sandbox
# is cancelled, and docker restarts on the new one
docker_before = show("InvocationID", "docker.service")
members_before = show("InvocationID", members)
push("member-sandbox", member_job)
box.wait_until_succeeds(f"{in_job} test -e /tmp/started", timeout=300)
# the timer's pass may come first: a loaded runner's switch took 50 s and
# its pass drained, so the test's own pass found nothing left to drain
since = journal_cursor(box)
box.succeed(f"{base}/specialisation/sandbox/bin/switch-to-configuration test >&2")
assert show("InvocationID", "docker.service") == docker_before, (
    "the switch restarted docker"
)
start_fresh(box, "dd-runner-restart.service")
said = journal_since(box, "dd-runner-restart.service", since)
assert "docker.service: its sandbox changed or is not known; draining" in said, said
wait_run(5, "failure", "cancelled")
restarted("docker.service", docker_before, timeout=240)
assert "--platform=systrap" in box.succeed("docker info --format '{{json .Runtimes}}'")
restarted(members, members_before, timeout=240)

# Which sandbox docker runs is not known (its record is gone): that is a
# change, not a pass. The job is cancelled and docker restarts; its start
# records the sandbox again, and the pass after leaves it be.
docker_before = show("InvocationID", "docker.service")
members_before = show("InvocationID", members)
push("member-unknown", member_job)
box.wait_until_succeeds(f"{in_job} test -e /tmp/started", timeout=300)
since = journal_cursor(box)
box.succeed("rm /run/dd-runner-restart/docker.service.sandbox")
start_fresh(box, "dd-runner-restart.service")
said = journal_since(box, "dd-runner-restart.service", since)
assert "docker.service: its sandbox changed or is not known; draining" in said, said
wait_run(6, "failure", "cancelled")
restarted("docker.service", docker_before, timeout=240)
restarted(members, members_before, timeout=240)
box.succeed("test -s /run/dd-runner-restart/docker.service.sandbox")
inv = start_fresh(box, "dd-runner-restart.service")
said = journal_of(box, "dd-runner-restart.service", inv)
assert "restarting" not in said and "draining" not in said, said
