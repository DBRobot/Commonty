# The photos test: museum up on loopback alone, its sessions kept as
# hashes, and the gate's side of Photos: the page that hands a browser
# to the app, its settings without the fleet's code, and the code filled
# in for her own address only, by session or by a device's token.
import hashlib
import json

from waits import journal_has, start_fresh

start_all()


def serving(unit, check, timeout):
    # a unit that has failed will never answer: stop now, with its journal
    def up(_):
        if box.execute(f"systemctl is-failed -q {unit}")[0] == 0:
            journal = box.execute(f"journalctl -b -u {unit} -o cat | tail -n 30")[1]
            raise Exception(f"{unit} failed:\n{journal}")
        return box.execute(check)[0] == 0

    with box.nested(f"waiting for {unit} to answer: {check}"):
        retry(up, timeout)


box.wait_for_unit("postgresql.service")
box.wait_for_unit("ente.service")
serving("ente.service", "curl -sf http://127.0.0.1:8080/ping", 120)
# museum's api and metrics on loopback, as the module's edit has them
listening = box.succeed("ss -Htln")
assert "127.0.0.1:8080" in listening and "127.0.0.1:2112" in listening, listening
for anywhere in (
    "*:8080",
    "0.0.0.0:8080",
    "[::]:8080",
    "*:2112",
    "0.0.0.0:2112",
    "[::]:2112",
):
    assert anywhere not in listening, listening
# the health check finds it well and leaves it be
# a run of its own: the timer's runs are not this check
run = start_fresh(box, "ente-health.service")
assert not journal_has(box, "ente-health.service", run, "restarting"), (
    "museum restarted"
)

box.wait_for_unit("nginx.service")
box.wait_for_open_port(4181)
site = "https://photos.test.invalid"
curl = "curl -sk --resolve photos.test.invalid:443:127.0.0.1"

# Ente's client comes with the app: the page here hands a browser to it,
# signed in or not, and holds no code that opens anything
page = box.succeed(f"{curl} -f {site}/_dd/photos")
assert "use-app.js" in page and "dd_web" not in page, page

# sarah and tom, members, each with a session as the app's browser has
dirs = "--directory http://127.0.0.1:4181/_dd/directory"
box.succeed("mkdir -p /root/fleet/fleet")
for who in ("sarah", "tom"):
    box.succeed(
        f"DD_KEYRING_FILE=/root/{who}.json {nix['dd']} identity new --born 1990-01 --agree-terms --name {who} {dirs}"
    )
    box.succeed(
        f"DD_KEYRING_FILE=/root/{who}.json {nix['dd']} member add {who} --repo /root/fleet {dirs} --confirm $(DD_KEYRING_FILE=/root/{who}.json {nix['dd']} identity show {dirs} | sed -n 's/^member id: *//p')"
    )
box.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    '"$(jq -c . /root/fleet/fleet/members.json)" '
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)


def session(who):
    token = box.succeed(f"DD_KEYRING_FILE=/root/{who}.json {nix['dd']} token").strip()
    head = box.succeed(
        f"curl -s -D - -o /dev/null -X POST --data-urlencode token={token} http://127.0.0.1:4181/_dd/app/signin"
    )
    return (
        next(h for h in head.splitlines() if h.lower().startswith("set-cookie:"))
        .split(":", 1)[1]
        .split(";")[0]
        .strip()
    )


sarah = f"-H 'Cookie: {session('sarah')}'"
tom = f"-H 'Cookie: {session('tom')}'"
as_json = "-H 'content-type: application/json'"

# what the app's Photos page needs: her address at museum, museum on this
# site's own name, and never the fleet's code
assert "use-app.js" in box.succeed(f"{curl} -f {sarah} {site}/_dd/photos")
cfg = json.loads(box.succeed(f"{curl} -f -X POST {sarah} {site}/_dd/photos/config"))
assert cfg["email"] == "sarah@users.test.invalid", cfg
assert cfg["accountApi"] == "/_dd/museum", cfg
assert "123456" not in json.dumps(cfg), cfg

# a code asked for her address, through nginx to museum
box.succeed(
    f'{curl} -f -X POST {as_json} -d \'{{"email": "sarah@users.test.invalid", "purpose": "signup"}}\' '
    f"{site}/_dd/museum/users/ott"
)
# tom cannot have the gate fill in the code for her address
code = box.succeed(
    f"{curl} -o /dev/null -w '%{{http_code}}' -X POST {tom} {as_json} "
    f'-d \'{{"email": "sarah@users.test.invalid"}}\' {site}/_dd/museum/users/verify-email'
).strip()
assert code == "403", code
# nor with his device's token, which is how the app's Photos page asks
tom_token = box.succeed(f"DD_KEYRING_FILE=/root/tom.json {nix['dd']} token").strip()
code = box.succeed(
    f"{curl} -o /dev/null -w '%{{http_code}}' -X POST -H 'Authorization: Bearer {tom_token}' {as_json} "
    f'-d \'{{"email": "sarah@users.test.invalid"}}\' {site}/_dd/museum/users/verify-email'
).strip()
assert code == "403", code
# she can: the gate adds the code, museum makes her account and a session
made = json.loads(
    box.succeed(
        f"{curl} -f -X POST {sarah} {as_json} "
        f'-d \'{{"email": "sarah@users.test.invalid"}}\' {site}/_dd/museum/users/verify-email'
    )
)
assert made["id"] and made["token"], made

# museum keeps the session as its sha256, and still knows it by the token
kept = box.succeed(
    "sudo -u postgres psql -d ente -tAc 'select token from tokens'"
).split()
assert kept == [hashlib.sha256(made["token"].encode()).hexdigest()], kept
box.succeed(
    f"curl -sf -H 'X-Auth-Token: {made['token']}' http://127.0.0.1:8080/users/session-validity/v2"
)

# What Mail does in Photos, against museum and a store: an account, a file
# saved into the album From Mail as Ente's apps save one, read back whole
# with its small picture, and a link made to it (client/ente, gallery.rs)
box.wait_for_unit("garage.service")
box.wait_for_open_port(3901)
node = box.succeed("garage node id -q").split("@")[0].strip()
box.succeed(
    f"garage layout assign -z here -c 1G {node} && garage layout apply --version 1"
)
box.wait_until_succeeds("garage bucket list", timeout=60)
box.succeed(
    "garage key import --yes GK0123456789abcdef01234567 "
    "1111111111111111111111111111111111111111111111111111111111111111"
    " && garage bucket create ente"
    " && garage bucket allow --read --write --owner ente --key GK0123456789abcdef01234567"
)
out = box.succeed(
    f"{nix['roundtrip']} http://127.0.0.1:8080 mail-check@users.test.invalid "
    "a-long-test-password 123456 2>&1"
)
print(out)
assert "roundtrip ok" in out, out
assert "https://albums.test.invalid" in out, out

# Nobody is museum's admin, and its admin api answers nobody through nginx
api = "curl -sk --resolve api.test.invalid:443:127.0.0.1 https://api.test.invalid"
assert box.succeed(f"{api}/admin/users -o /dev/null -w '%{{http_code}}'") == "404"
assert (
    box.succeed(
        f"{curl} -o /dev/null -w '%{{http_code}}' {site}/_dd/museum/admin/users"
    )
    == "404"
)


# and museum itself takes nobody as its admin: not a member, not the
# first account it ever made, even asked directly
for t in (made["token"],):
    assert (
        box.succeed(
            f"curl -s -o /dev/null -w '%{{http_code}}' -H 'X-Auth-Token: {t}' http://127.0.0.1:8080/admin/users"
        )
        == "401"
    )


def ente_sql(q):
    return box.succeed(f'sudo -u ente psql -d ente -tAc "{q}"').strip()


# what she keeps is counted from museum's own tables, as museum's own user
box.wait_until_succeeds(
    '[ -n "$(sudo -u postgres psql -d ente -tAc "select dd_email from users where dd_email = \'sarah@users.test.invalid\'")" ]',
    timeout=90,
)
box.succeed("systemctl start dd-photos-usage.service")
assert "sarah@users.test.invalid\t" in box.succeed(
    "cat /var/lib/dd-photos-jobs/usage.tsv"
)
# and her limit set from what the ledger worked out, by the same user
box.succeed(
    "install -d -o dd-storage -g dd-storage -m 0750 /var/lib/dd-storage && "
    "printf 'sarah@users.test.invalid\\t123456789\\n' > /var/lib/dd-storage/photo-limits.tsv && "
    "chown dd-storage:dd-storage /var/lib/dd-storage/photo-limits.tsv"
)
box.succeed("systemctl start dd-photos-limits.service")
assert (
    ente_sql(
        "select s.storage from subscriptions s join users u using (user_id) where u.dd_email = 'sarah@users.test.invalid'"
    )
    == "123456789"
)

# a member who left: listed by the box, deleted by museum itself, and her
# address gone with her account, so no new keys bring it back
ente_sql("insert into dd_delete (email) values ('sarah@users.test.invalid')")
box.wait_until_succeeds(
    '[ "$(sudo -u ente psql -d ente -tAc "select count(*) from users where dd_email = \'sarah@users.test.invalid\'")" = 0 ]',
    timeout=120,
)
assert ente_sql("select count(*) from dd_delete") == "0"
