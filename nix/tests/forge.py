# The forge test: it comes up, its admin exists, main is protected by every ci job.

box.wait_for_unit("forgejo.service")
# forgejo serves on a socket, not a port: nothing else on the box may
# name itself to it through X-WEBAUTH-USER
box.wait_until_succeeds("test -S %s" % nix["sock"], timeout=120)
box.fail("curl -s -m 3 -o /dev/null http://127.0.0.1:%d/" % nix["port"])


# the setup units are oneshots: a unit that has not run yet also says
# Result=success, so wait for one that has finished a run
def done(unit):
    box.wait_until_succeeds(
        '[ -n "$(systemctl show -p ExecMainExitTimestamp --value %s)" ] && systemctl show -p Result --value %s | grep -qx success'
        % (unit, unit),
        timeout=180,
    )


done("forgejo-admin.service")
# with no repository yet, protection has nothing to do and says so
done("forgejo-protection.service")
box.succeed("journalctl -u forgejo-protection | grep -q 'nothing to protect'")

# the repository is the one thing the forge does not make for itself (it is
# pushed to); here it is made through the api, then the protection applies
box.succeed(
    "curl -sf --unix-socket %s -X POST -H 'X-WEBAUTH-USER: %s' -H 'content-type: application/json' "
    '-d \'{"name":"commonty","default_branch":"main","auto_init":true}\' '
    "http://forgejo/api/v1/user/repos >/dev/null" % (nix["sock"], nix["admin"])
)
box.succeed("systemctl restart forgejo-protection.service")
done("forgejo-protection.service")

# the admin, by the name the role gave (this is what the quoting bug broke)
users = box.succeed(
    "curl -sf --unix-socket %s -H 'X-WEBAUTH-USER: %s' http://forgejo/api/v1/admin/users"
    % (nix["sock"], nix["admin"])
)
assert nix["admin"] in users, users
assert '"is_admin":true' in users, "the admin is an admin"

# main is protected: by ci_ok, the job that waits on every other
# (.forgejo/workflows/entry_point.yml), on a branch up to date with main
prot = box.succeed(
    "curl -sf --unix-socket %s -H 'X-WEBAUTH-USER: %s' http://forgejo/api/v1/repos/%s/commonty/branch_protections/main"
    % (nix["sock"], nix["admin"], nix["admin"])
)
import json

p = json.loads(prot)
assert p["enable_status_check"], p
assert p["status_check_contexts"] == ["entry_point / ci_ok (pull_request)"], p[
    "status_check_contexts"
]
assert p["block_on_outdated_branch"], p
assert p["required_approvals"] == 0
# and nobody pushes main directly, the admin included
assert not p["enable_push"], p

# no private repositories: the forge's database refuses one, made or changed
box.wait_for_unit("dd-forge-rules.service")
box.fail(
    "curl -sf --unix-socket %s -X POST -H 'X-WEBAUTH-USER: %s' -H 'content-type: application/json' "
    '-d \'{"name":"secret","private":true}\' '
    "http://forgejo/api/v1/user/repos" % (nix["sock"], nix["admin"])
)
box.fail(
    "curl -sf --unix-socket %s -X PATCH -H 'X-WEBAUTH-USER: %s' -H 'content-type: application/json' "
    "-d '{\"private\":true}' http://forgejo/api/v1/repos/%s/commonty"
    % (nix["sock"], nix["admin"], nix["admin"])
)
assert not json.loads(
    box.succeed(
        "curl -sf --unix-socket %s -H 'X-WEBAUTH-USER: %s' http://forgejo/api/v1/repos/%s/commonty"
        % (nix["sock"], nix["admin"], nix["admin"])
    )
)["private"]

# a runner registered for every repository is the admin's, unless all it
# runs is members' sandboxed jobs
sql = "sudo -u postgres psql -d forgejo -tAc"
box.succeed(
    f'{sql} "insert into action_runner (uuid, name, owner_id, repo_id, token_hash, agent_labels) values '
    "('00000000-0000-0000-0000-000000000001', 'host', 0, 0, 'h1', '[\\\"nix\\\"]'), "
    "('00000000-0000-0000-0000-000000000002', 'members', 0, 0, 'h2', '[\\\"members\\\"]')\""
)
admin_id = box.succeed(
    f'{sql} "select id from \\"user\\" where lower_name = lower(\'{nix["admin"]}\')"'
).strip()
assert (
    box.succeed(
        f"{sql} \"select owner_id from action_runner where name = 'host'\""
    ).strip()
    == admin_id
)
assert (
    box.succeed(
        f"{sql} \"select owner_id from action_runner where name = 'members'\""
    ).strip()
    == "0"
)

# a run a restart killed is run again: the timer is set, and a pass over a
# forge with no runs finds nothing to do and fails nothing
box.succeed("systemctl list-timers --all | grep -q dd-ci-resume")
box.succeed("systemctl start dd-ci-resume.service")

# no ci secret in the repo: the cancel route needs none
secrets = box.succeed(
    "curl -sf --unix-socket %s -H 'X-WEBAUTH-USER: %s' http://forgejo/api/v1/repos/%s/commonty/actions/secrets"
    % (nix["sock"], nix["admin"], nix["admin"])
)
assert "DD_CI" not in secrets, secrets

# the cancel route takes any run id and acts on none that has not failed:
# a run that does not exist is looked up, not found, and left alone
box.wait_for_open_port(3003)
box.succeed(
    "test $(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:3003/_dd/ci/cancel/1) = 202"
)
box.succeed(
    "test $(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:3003/_dd/ci/other) = 404"
)
box.wait_until_succeeds(
    "journalctl -u dd-ci-cancel | grep -q 'run 1: not found'", timeout=30
)

# The Git pages update as things happen: a commit status the forge records
# reaches a member watching the repository, through the gate, at once. The
# trigger that announces it is put in after every forge start.
import json

box.wait_for_unit("dd-forge-notify.service")
env = "DD_KEYRING_FILE=/root/keys.json"
dirs = "--directory http://127.0.0.1:4181/_dd/directory"
box.wait_for_open_port(4181)
box.succeed(
    f"{env} {nix['dd']} identity new --born 1990-01 --agree-terms --name sarah {dirs}"
)
box.succeed("mkdir -p /root/fleet/fleet")
box.succeed(f"{env} {nix['dd']} member add sarah --repo /root/fleet {dirs}")
box.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    '"$(jq -c . /root/fleet/fleet/members.json)" '
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)
token = box.succeed(f"{env} {nix['dd']} token").strip()
head = box.succeed(
    f"curl -s -D - -o /dev/null -X POST --data-urlencode token={token} http://127.0.0.1:4181/_dd/app/signin"
)
jar = (
    [l for l in head.splitlines() if l.lower().startswith("set-cookie:")][0]
    .split(":", 1)[1]
    .split(";")[0]
    .strip()
)

forge = (
    "curl -sf --unix-socket %s -H 'X-WEBAUTH-USER: %s' http://forgejo/api/v1/repos/%s/commonty"
    % (nix["sock"], nix["admin"], nix["admin"])
)
repo = json.loads(box.succeed(forge))
sha = json.loads(box.succeed(forge + "/branches/main"))["commit"]["id"]
# nobody signed in hears nothing
box.succeed(
    f"test $(curl -s -o /dev/null -w '%{{http_code}}' 'http://127.0.0.1:4181/_dd/git/events?repo={repo['id']}') = 401"
)
# sarah watches; the gate's listener may still be connecting, so the status
# is recorded until one arrives
box.succeed(
    f"(curl -sN --max-time 60 -H 'Cookie: {jar}' 'http://127.0.0.1:4181/_dd/git/events?repo={repo['id']}' > /tmp/events &)"
)
box.wait_until_succeeds(
    f"{forge.replace('curl -sf', 'curl -sf -X POST -H content-type:application/json')}/statuses/{sha} "
    '-d \'{"state": "pending", "context": "live test"}\' >/dev/null && sleep 1 && grep -q \'"kind":"status"\' /tmp/events',
    timeout=45,
)
box.succeed(f'grep -q \'"sha":"{sha}"\' /tmp/events')
# the gate's login may connect, and reads no table
box.succeed(
    "sudo -u dd-verify psql -h /run/postgresql -d forgejo -tAc 'select 1' | grep -qx 1"
)
# psql fails here, as it should; the pipe must not take that for the test's
box.succeed(
    "(sudo -u dd-verify psql -h /run/postgresql -d forgejo -tAc 'select count(*) from repository' 2>&1 || true) | grep -q 'permission denied'"
)

# The public door (nix/modules/forge/door.nix) asks the forge as nobody: no
# header, no cookie. What nobody is shown here is what the world is shown
# at code.<domain>; nginx-door.py checks the door lets nothing else through.
nobody = "curl -s --unix-socket %s" % nix["sock"]
as_admin = (
    "curl -sf --unix-socket %s -H 'X-WEBAUTH-USER: %s' -H 'content-type: application/json'"
    % (
        nix["sock"],
        nix["admin"],
    )
)


def status(path):
    return box.succeed(
        f"{nobody} -o /dev/null -w '%{{http_code}}' 'http://forgejo{path}'"
    ).strip()


# a public repository: read, and cloned, by nobody
assert status(f"/api/v1/repos/{nix['admin']}/commonty") == "200"
assert (
    status(f"/{nix['admin']}/commonty.git/info/refs?service=git-upload-pack") == "200"
)
# a repository nobody may see is not there for nobody, by name or in a
# search: an organisation only members see, and its repository
box.succeed(
    f'{as_admin} -X POST -d \'{{"username":"inner","visibility":"limited"}}\' http://forgejo/api/v1/orgs >/dev/null'
)
box.succeed(
    f'{as_admin} -X POST -d \'{{"name":"hidden","auto_init":true}}\' http://forgejo/api/v1/orgs/inner/repos >/dev/null'
)
assert status("/api/v1/repos/inner/hidden") == "404"
for path in ["/api/v1/repos/search?q=hidden", "/api/v1/users/inner/repos"]:
    found = box.succeed(f"{nobody} 'http://forgejo{path}'")
    assert "inner/hidden" not in found, (path, found)
# git says "sign in" (401) for it; the door turns that into a 404 too
assert status("/inner/hidden.git/info/refs?service=git-upload-pack") in (
    "401",
    "404",
)
# an account's own address is nobody's, whatever it is set to
box.succeed(
    f"{sql} \"update \\\"user\\\" set email = 'tester@example.org', keep_email_private = false where lower_name = lower('{nix['admin']}')\""
)
for path in [
    f"/api/v1/repos/{nix['admin']}/commonty",
    f"/api/v1/users/{nix['admin']}/repos",
    "/api/v1/repos/search?q=commonty",
]:
    said = box.succeed(f"{nobody} 'http://forgejo{path}'")
    assert "example.org" not in said, (path, said)
