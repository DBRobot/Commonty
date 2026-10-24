# The journeys (site-journeys.nix): the box set up as a member finds it -
# their account, their library with a file in it, a repository - then the
# browser on the box walks each service's one main thing (site/journeys.py).
import json

from waits import start_fresh, wait_fetch_has, wait_socket, wait_unit_active

start_all()

d = nix["domain"]
dd = nix["dd"]
env = "DD_KEYRING_FILE=/root/mia.json"
dirs = "--directory http://127.0.0.1:4181/_dd/directory"

# museum's garage: its layout, then its bucket and key
box.wait_for_unit("garage.service")
wait_socket(box, 3901)
start_fresh(box, "garage-setup.service")
box.wait_until_succeeds(
    "garage layout show > /tmp/l && grep -q 'Current cluster layout version: [1-9]' /tmp/l"
)
start_fresh(box, "garage-setup.service")
# the libraries' bucket and the gate's keys to it, in the box's own store
box.wait_for_unit("dd-store-setup.service")
box.wait_for_unit("dd-library-keys.service")
wait_socket(box, 4181)

# mia: an account, on the release's member list, the box's owner
box.succeed(f"{env} {dd} identity new --born 1990-01 --agree-terms --name mia {dirs}")
box.succeed("mkdir -p /root/fleet/fleet")
box.succeed(
    f"{env} {dd} member add mia --repo /root/fleet {dirs} --confirm "
    f"$({env} {dd} identity show {dirs} | sed -n 's/^member id: *//p')"
)
box.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    '"$(jq -c . /root/fleet/fleet/members.json)" '
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
wait_socket(box, 4181)

# her library, a file and a film in it, put there by dd (the same crypt as
# the pages)
out = box.succeed(f"{env} {dd} library new {dirs}")
lib = out.split("library ")[1].split(":")[0].strip()
key = box.succeed(f"{env} {dd} library key {lib} {dirs}").strip()
box.succeed("printf 'Otters hold hands while they sleep.\\n' > /root/otters.txt")
box.succeed(
    f"{env} {dd} library put {lib} /root/otters.txt --as Files/otters.txt {dirs}"
)
box.succeed("head -c 4096 /dev/zero > /root/film.mp4")
box.succeed(
    f"{env} {dd} library put {lib} /root/film.mp4 --as 'Movies/River Otters (2021).mp4' {dirs}"
)
token = box.succeed(f"{env} {dd} token").strip()
assert "\n" not in token, token
# the device's token opens her own answers, through nginx as the app asks
me = box.succeed(
    f"curl -s -o /dev/null -w '%{{http_code}}' --cacert {nix['ca']} "
    f"-H 'Authorization: Bearer {token}' https://files.{d}/_dd/me"
).strip()
direct = box.succeed(
    f"curl -s -o /dev/null -w '%{{http_code}}' -H 'Authorization: Bearer {token}' http://127.0.0.1:4181/_dd/me"
).strip()
assert (me, direct) == ("200", "200"), (
    f"/_dd/me with her device token: through nginx {me}, at the gate {direct}"
)
box.succeed(
    "cat > /root/app-state.json <<'EOF'\n"
    + json.dumps(
        {"token": token, "user": "mia", "libraries": [{"id": lib, "key": key}]}
    )
    + "\nEOF"
)

# the demo's photos: its account at museum, with the password the gate
# holds, and photos saved into it as Ente's apps save them: a real picture
# among them, which the gallery can show
box.succeed(
    "garage key import --yes GK0123456789abcdef01234568 "
    "1111111111111111111111111111111111111111111111111111111111111111"
    " && garage bucket create ente"
    " && garage bucket allow --read --write --owner ente --key GK0123456789abcdef01234568"
)
box.wait_for_unit("ente.service")
wait_socket(box, 8080)
out = box.succeed(
    f"{nix['roundtrip']} http://127.0.0.1:8080 demo@users.{d} a-long-demo-password 123456 {nix['picture']} 2>&1"
)
assert "roundtrip ok" in out, out

# her repository, made as the forge makes one for anyone the gate names
box.wait_for_unit("forgejo.service")
wait_socket(box, "/run/forgejo/forgejo.sock")
box.succeed(
    "curl -sf --unix-socket /run/forgejo/forgejo.sock -H 'X-WEBAUTH-USER: mia' "
    "-H 'content-type: application/json' "
    '-d \'{"name":"notes","auto_init":true,"readme":"Default","description":"where the otters are written down"}\' '
    "http://forgejo/api/v1/user/repos >/dev/null"
)

# the figures: Thanos has this box's series
wait_unit_active(box, "thanos-query.service")
wait_fetch_has(
    box, "http://127.0.0.1:10903/api/v1/query?query=up%7Bbox%3D%22box%22%7D", '"value"'
)
# the model and the games manager answer
wait_socket(box, "/run/dd-llm/llm.sock")
box.wait_for_unit("nginx.service")
box.wait_for_unit("dd-games.service")

# the browser: each journey, its result kept
status, _ = box.execute(
    f"cd /tmp && PLAYWRIGHT_BROWSERS_PATH={nix['browsers']} FONTCONFIG_FILE={nix['fonts']} "
    f"SITE_CA={nix['ca']} {nix['python']} {nix['journeys']} "
    f"{d} {nix['app']} {nix['carried']} {nix['web']} /root/app-state.json /tmp/journeys.json {nix['picture']} "
    f"> /tmp/journeys.log 2>&1"
)
if status != 0:
    print(
        box.execute(
            "echo '--- app'; tail -40 /tmp/app.log; echo '--- gate'; journalctl -u dd-verify --no-pager -o cat | tail -40; "
            "echo '--- nginx, what did not answer 2xx/3xx'; grep -vE '\" [23][0-9][0-9] ' /var/log/nginx/access.log | cut -c1-300 | tail -40"
        )[1]
    )
    # a picture of each page a journey stopped on, in the build's output
    if box.execute("test -d /tmp/journeys-shots")[0] == 0:
        box.copy_from_machine("/tmp/journeys-shots")
    jlog = box.execute("cat /tmp/journeys.log")[1]
    print(jlog)
    # the journeys that failed, last, where the build's tail shows them
    raise AssertionError(
        "site-journeys failed\n"
        + "\n".join(
            ln
            for ln in jlog.splitlines()
            if ln.startswith(("failed:", "site-journeys:"))
        )
    )
print(box.succeed("cat /tmp/journeys.log"))

# what the journeys left behind, as the box keeps it
listing = box.succeed(f"{env} {dd} library ls {lib} {dirs}")
assert any(line.endswith(".ics") for line in listing.split()), listing
assert any(line.endswith(".vcf") for line in listing.split()), listing
model = [json.loads(line) for line in box.succeed("cat /tmp/model.log").splitlines()]
asked = [m for m in model if m["path"].startswith("/v1/chat/completions")]
assert asked and all(m["auth"] for m in asked), model
assert all(not m["cookie"] or "dd_session" not in m["cookie"] for m in model), model
assert "Otters" in asked[-1]["body"] or "otters" in asked[-1]["body"], asked[-1]
