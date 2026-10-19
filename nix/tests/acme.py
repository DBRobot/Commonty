from waits import start_fresh

box1.start()
for unit in [
    "challtestsrv",
    "pebble",
    "fake-cloudflare",
    "dns-worker",
    "dd-acme.socket",
]:
    box1.wait_for_unit(unit)


def worker_up(_):
    # wrangler that has exited (a Worker that will not load) never answers
    if box1.execute("systemctl is-failed -q dns-worker.service")[0] == 0:
        journal = box1.execute("journalctl -b -u dns-worker -o cat | tail -n 30")[1]
        raise Exception(f"the Worker did not start:\n{journal}")
    return box1.execute("curl -s -o /dev/null http://127.0.0.1:8787/")[0] == 0


with box1.nested("waiting for the Worker on 127.0.0.1:8787"):
    retry(worker_up, 120)
box1.wait_until_succeeds(
    "curl -s -o /dev/null --cacert /etc/ssl/certs/ca-certificates.crt https://localhost:14000/dir",
    timeout=60,
)

with subtest("only lego's user can ask for a challenge"):
    out = box1.fail(
        "runuser -u nginx -- socat -t 5 - UNIX-CONNECT:/run/dd-acme.sock </dev/null 2>&1"
    )
    assert "Permission denied" in out, out

with subtest("the box's certificate, issued through the Worker"):
    # boot never ordered one (acme.nix): a boot-time order raced wrangler
    box1.succeed(
        'test -z "$(systemctl show -P InvocationID acme-order-renew-test.invalid.service)"'
    )
    start_fresh(box1, "acme-order-renew-test.invalid.service")
    cert = "/var/lib/acme/test.invalid/cert.pem"
    issuer = box1.succeed(f"openssl x509 -in {cert} -noout -issuer")
    assert "Pebble" in issuer, issuer
    sans = box1.succeed(
        f"openssl x509 -in {cert} -noout -ext subjectAltName | tail -n +2"
    ).strip()
    got = sorted(s.strip().removeprefix("DNS:") for s in sans.split(","))
    want = sorted(
        ["home.test.invalid", "files.test.invalid", "demo-files.test.invalid"]
    )
    assert got == want, f"{got} != {want}"

with subtest("each name was asked for by the helper, and cleaned up"):
    records = box1.succeed("curl -s http://127.0.0.1:8899/records")
    assert records.strip() == "[]", records

with subtest("a name the box does not serve is refused at the box"):
    out = box1.succeed(
        "printf 'present _acme-challenge.elsewhere.example. %s\\n' AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        " | runuser -u acme -- socat -t 30 - UNIX-CONNECT:/run/dd-acme.sock"
    )
    assert out.startswith("not a name this box serves"), out
