# The vault test: Vaultwarden up on postgres and garage, the gate's issuer
# answering for it, and the Send page open to anyone.
import json

box.wait_for_unit("garage.service")
box.wait_for_open_port(3901)
box.succeed("systemctl restart garage-setup.service")
box.wait_until_succeeds(f"set -a; . {nix['rpc']}; garage layout show > /tmp/l && grep -q 'Current cluster layout version: [1-9]' /tmp/l")
box.succeed("systemctl restart garage-setup.service")  # the bucket and its key, now that the layout exists

box.succeed("systemctl restart vaultwarden.service")
box.wait_for_unit("vaultwarden.service")
box.wait_until_succeeds("curl -sf http://127.0.0.1:8222/alive", timeout=120)

# its files are in the bucket, not on the box: the key it makes at first
# start is there
s3 = (
    "AWS_ACCESS_KEY_ID=GK0123456789abcdef01234567 "
    "AWS_SECRET_ACCESS_KEY=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef "
    "AWS_DEFAULT_REGION=us-east-1 aws --endpoint-url http://127.0.0.1:3900"
)
box.wait_until_succeeds(f"{s3} s3 ls s3://vaultwarden/data/ | grep -q rsa_key", timeout=60)
box.fail("ls /var/lib/vaultwarden/rsa_key.pem")

# and its records in postgres, where the hourly dumps take them from
box.succeed("sudo -u postgres psql -d vaultwarden -tAc 'select count(*) from users'")
box.succeed("systemctl start postgresqlBackup-vaultwarden.service")

# sign-in is Commonty's only: no password logins, the gate as issuer
# a password login, as a real client sends it, is turned away: sign-in is
# Commonty's only
out = box.succeed(
    "curl -s -X POST http://127.0.0.1:8222/identity/connect/token "
    "-H 'Bitwarden-Client-Version: 2026.6.0' -H 'Bitwarden-Client-Name: web' -H 'Device-Type: 9' "
    "-d grant_type=password -d username=sarah@test.invalid -d password=x "
    "-d scope='api offline_access' -d client_id=web -d deviceType=9 -d deviceIdentifier=t -d deviceName=t"
)
assert "access_token" not in out, out
assert "sso" in out.lower() or "SSO" in out, out

box.wait_for_open_port(4181)
disc = json.loads(box.succeed("curl -sf http://127.0.0.1:4181/_dd/oidc/.well-known/openid-configuration"))
assert disc["issuer"] == "http://127.0.0.1:4181/_dd/oidc", disc
assert "S256" in disc["code_challenge_methods_supported"], disc
keys = json.loads(box.succeed("curl -sf http://127.0.0.1:4181/_dd/oidc/jwks"))
assert keys["keys"], keys
# nobody signed in gets no code, only the way to sign in
code = box.succeed(
    "curl -s -o /dev/null -w '%{http_code}' "
    "'http://127.0.0.1:4181/_dd/oidc/authorize?client_id=vaultwarden&response_type=code"
    "&redirect_uri=https://vault.test.invalid/identity/connect/oidc-signin&state=x"
    "&code_challenge=abc&code_challenge_method=S256&scope=openid'"
).strip()
assert code in ("302", "303", "401"), code
# a code nobody was given buys nothing
box.fail(
    "curl -sf -X POST http://127.0.0.1:4181/_dd/oidc/token "
    "-d grant_type=authorization_code -d code=made-up -d client_id=vaultwarden "
    "-d client_secret=test-oidc-secret -d code_verifier=x"
)

# the Send page and its script, for anyone
page = box.succeed("curl -sf http://127.0.0.1:4181/_dd/send")
assert "send.js" in page, page
box.succeed("curl -sf http://127.0.0.1:4181/_dd/static/send.js | grep -q bitwarden-send")
box.succeed("curl -sf http://127.0.0.1:4181/_dd/static/vault-bar.js -o /dev/null")

# the breach list is fetched on a timer, from the internet this test has not
box.succeed("systemctl list-timers dd-pwned.timer | grep -q dd-pwned")

# A member signs in to Passwords through Commonty, the whole way: Vaultwarden
# sends them to the gate, the gate (with their session) hands back a code,
# Vaultwarden trades it for the gate's signed token and makes their account.
import base64
import hashlib
import secrets
import urllib.parse

env = "DD_KEYRING_FILE=/root/keys.json"
dirs = "--directory http://127.0.0.1:4181/_dd/directory"
box.succeed(f"{env} {nix['dd']} identity new --name sarah {dirs}")
box.succeed("mkdir -p /root/fleet/fleet")
box.succeed(f"{env} {nix['dd']} member add sarah --repo /root/fleet {dirs}")
box.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    "\"$(jq -c . /root/fleet/fleet/members.json)\" "
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)

# the app's way in: a device token for a session, as the app's browser does
token = box.succeed(f"{env} {nix['dd']} token").strip()
head = box.succeed(f"curl -s -D - -o /dev/null -X POST --data-urlencode token={token} http://127.0.0.1:4181/_dd/app/signin")
jar = [l for l in head.splitlines() if l.lower().startswith("set-cookie:")][0].split(":", 1)[1].split(";")[0].strip()
assert jar.startswith("dd_session="), head

# what a Bitwarden client does: its own pkce, then Vaultwarden's authorize
verifier = secrets.token_urlsafe(48)
challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
back = "https://vault.test.invalid/sso-connector.html"
q = urllib.parse.urlencode({
    "client_id": "web", "redirect_uri": back, "response_type": "code",
    "scope": "api offline_access", "state": "client-state", "code_challenge": challenge,
    "code_challenge_method": "S256", "domain_hint": "test.invalid",
})
to_gate = box.succeed(f"curl -s -c /root/vw.jar -o /dev/null -w '%{{redirect_url}}' 'http://127.0.0.1:8222/identity/connect/authorize?{q}'").strip()
assert to_gate.startswith("http://127.0.0.1:4181/_dd/oidc/authorize?"), to_gate
# the gate knows sarah by her session and sends her back with a code
to_vault = box.succeed(f"curl -s -o /dev/null -w '%{{redirect_url}}' -H 'Cookie: {jar}' '{to_gate}'").strip()
assert to_vault.startswith("https://vault.test.invalid/identity/connect/oidc-signin?code="), to_vault
signin = to_vault.replace("https://vault.test.invalid", "http://127.0.0.1:8222")
to_client = box.succeed(f"curl -s -b /root/vw.jar -o /dev/null -w '%{{redirect_url}}' '{signin}'").strip()
assert to_client.startswith(back), to_client
code = urllib.parse.parse_qs(urllib.parse.urlparse(to_client).query)["code"][0]
# and the client's code buys a session in Passwords: Vaultwarden checked the
# gate's signed token to get here
got = box.succeed(
    "curl -s -X POST http://127.0.0.1:8222/identity/connect/token "
    "-H 'Bitwarden-Client-Version: 2026.6.0' -H 'Bitwarden-Client-Name: web' -H 'Device-Type: 9' "
    f"--data-urlencode grant_type=authorization_code --data-urlencode code={code} "
    f"--data-urlencode code_verifier={verifier} --data-urlencode redirect_uri={back} "
    "--data-urlencode client_id=web --data-urlencode 'scope=api offline_access' "
    "--data-urlencode deviceType=9 --data-urlencode deviceIdentifier=test-device --data-urlencode deviceName=test"
)
assert "access_token" in got, got
# her account is hers by her Commonty name, and holds no password yet: she
# sets the master password on her own device next
rows = box.succeed("sudo -u postgres psql -d vaultwarden -tAc \"select email from users\"").split()
assert rows == ["sarah@test.invalid"], rows

# Signed out of Passwords everywhere: the gate drops her name when a device
# or passkey of hers goes, or she signs out everywhere else; her session
# here stops working and her app cannot refresh it
tokens = json.loads(got)
api = f"curl -s -o /dev/null -w '%{{http_code}}' -H 'Authorization: Bearer {tokens['access_token']}' http://127.0.0.1:8222/api/accounts/revision-date"
assert box.succeed(api).strip() == "200"
box.succeed("systemctl show dd-verify.service -p Environment | grep -q VERIFY_ENDED=/run/dd-ended")
box.succeed("sudo -u dd-verify touch /run/dd-ended/sarah")
box.wait_until_succeeds("test ! -e /run/dd-ended/sarah", timeout=30)
box.wait_until_succeeds(f"test \"$({api})\" = 401", timeout=10)
again = box.succeed(
    "curl -s -X POST http://127.0.0.1:8222/identity/connect/token "
    f"--data-urlencode grant_type=refresh_token --data-urlencode refresh_token={tokens['refresh_token']} "
    "--data-urlencode client_id=web"
)
assert "access_token" not in again, again
# a name that is not one is dropped and goes no further
box.succeed("sudo -u dd-verify touch '/run/dd-ended/-x'")
box.wait_until_succeeds("test ! -e '/run/dd-ended/-x'", timeout=30)
