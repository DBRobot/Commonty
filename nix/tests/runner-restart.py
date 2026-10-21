import base64
import json

from waits import start_fresh, wait_unit_active, journal_of

runner = nix["runner"]
admin = nix["admin"]
as_admin = (
    f"curl -sf --unix-socket {nix['sock']} -H 'X-WEBAUTH-USER: {admin}' "
    "-H 'content-type: application/json'"
)
api = f"http://forgejo/api/v1/repos/{admin}/commonty"


def show(prop, unit=runner):
    return box.succeed(f"systemctl show -P {prop} {unit}").strip()


def config_capacity():
    """the capacity in the config the running runner was started with"""
    pid = show("MainPID")
    args = box.succeed(f"tr '\\0' ' ' < /proc/{pid}/cmdline").split()
    conf = args[args.index("--config") + 1]
    return box.succeed(f"grep -o 'capacity: [0-9]*' {conf}").split()[1]


def run_of(number):
    runs = json.loads(box.succeed(f"{as_admin} '{api}/actions/runs?limit=20'"))
    return next(r for r in runs["workflow_runs"] if r["index_in_repo"] == number)


def wait_run(number, status):
    box.wait_until_succeeds(
        f"{as_admin} '{api}/actions/runs?limit=20' | jq -e "
        f"'.workflow_runs[] | select(.index_in_repo == {number}) | .status == \"{status}\"'",
        timeout=300,
    )


box.wait_for_unit("forgejo.service")
box.wait_until_succeeds(f"test -S {nix['sock']}", timeout=120)
box.wait_until_succeeds(
    '[ -n "$(systemctl show -P ExecMainExitTimestamp forgejo-admin.service)" ]',
    timeout=180,
)
box.wait_for_unit("nginx.service")
box.succeed(
    f'{as_admin} -X POST -d \'{{"name":"commonty","default_branch":"main","auto_init":true}}\' '
    "http://forgejo/api/v1/user/repos >/dev/null"
)

# the runner registers with a token the forge mints, as on a box
token = json.loads(
    box.succeed(
        f"{as_admin} http://forgejo/api/v1/admin/actions/runners/registration-token"
    )
)["token"]
box.succeed(f"umask 077; echo TOKEN={token} > /run/runner.env")
box.succeed(f"systemctl reset-failed {runner} || true; systemctl restart {runner}")
wait_unit_active(box, runner)
box.wait_until_succeeds(
    f"journalctl -u {runner} > /tmp/runner.log; grep -c 'declared successfully' /tmp/runner.log"
)
assert config_capacity() == "1"
# the runner on its first config is current: the restart unit leaves it be
start_fresh(box, "dd-runner-restart.service")
assert show("ActiveState") == "active"

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


# main is protected (forgejo-protection): each job is a push to a branch
def push(name):
    content = base64.b64encode(job.encode()).decode()
    box.succeed(
        f'{as_admin} -X POST -d \'{{"branch":"main","new_branch":"{name}","content":"{content}"}}\' '
        f"{api}/contents/.forgejo/workflows/{name}.yml >/dev/null"
    )


push("long")
box.wait_until_succeeds("test -e /run/job-gate/started-1", timeout=300)
before = show("InvocationID")

# the release: the runner's unit changes, the gate stops and starts
box.succeed(
    "/run/current-system/specialisation/next/bin/switch-to-configuration test >&2"
)
assert show("InvocationID") == before, "the switch restarted the runner"
assert show("ActiveState") == "active"
assert config_capacity() == "1"
# it is the new unit on disk, and the restart unit waits for the job
assert "capacity: 2" in box.succeed(
    f"cat $(systemctl show -P ExecStart {runner} | grep -o -- '--config [^ ;]*' | cut -d' ' -f2)"
)
inv = start_fresh(box, "dd-runner-restart.service")
assert "a job is running" in journal_of(box, "dd-runner-restart.service", inv)
assert show("InvocationID") == before
assert run_of(1)["status"] == "running", run_of(1)

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
box.wait_until_succeeds("test -e /run/job-gate/started-2", timeout=300)
mid = show("InvocationID")
box.succeed(f"systemctl restart --no-block {runner}")
box.wait_until_succeeds(
    f'[ "$(systemctl show -P ActiveState {runner})" = deactivating ]', timeout=30
)
box.succeed("touch /run/job-gate/go-2")
wait_run(2, "success")
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
