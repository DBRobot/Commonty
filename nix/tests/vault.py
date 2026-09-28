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
assert disc["issuer"] == "https://home.test.invalid/_dd/oidc", disc
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
