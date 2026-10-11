# The directory test: an entry published, a device admitted, a restart, and
# what a stranger with no credential is refused.

start_all()
for m in (a, b):
    m.wait_for_unit("dd-verify.service")
    m.wait_for_open_port(4181)

env = "DD_KEYRING_FILE=/root/laptop.json"
for box in ("a", "b"):
    client.succeed(f"{env}-{box} {nix['dd']} identity new --name sarah --directory http://{box}:4181/_dd/directory")

# the list is members': a stranger is refused it, sarah's device is not
for box in ("a", "b"):
    out = client.succeed(f"curl -s -o /dev/null -w '%{{http_code}}' http://{box}:4181/_dd/directory").strip()
    assert out == "401", f"{box}: anonymous list answered {out}"

# a: no reader key in the release yet, so one entry is still anyone's
a.succeed("curl -sf http://localhost:4181/_dd/directory/sarah")
# b: refused, and the same refusal whether or not the name is there
for name in ("sarah", "nobody"):
    out = client.succeed(f"curl -s -o /dev/null -w '%{{http_code}}' http://b:4181/_dd/directory/{name}").strip()
    assert out == "401", f"b: anonymous {name} answered {out}"

# her own device reads her entry (it sends its token), and admits a phone
show = client.succeed(f"{env}-b {nix['dd']} identity show --directory http://b:4181/_dd/directory")
assert "version 1" in show and "(yours)" in show, show
pub = client.succeed(f"DD_KEYRING_FILE=/root/phone.json {nix['dd']} device show | tail -1").strip()
client.succeed(f"{env}-b {nix['dd']} device admit {pub} --directory http://b:4181/_dd/directory")
show = client.succeed(f"{env}-b {nix['dd']} identity show --directory http://b:4181/_dd/directory")
assert "version 2" in show, show

# b restarts and still has sarah at version 2
b.succeed("systemctl restart dd-verify")
b.wait_for_open_port(4181)
b.wait_until_succeeds("grep -q '\"version\": 2' /var/lib/dd-verify/keys/sarah.json")
