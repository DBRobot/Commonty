box1.start()
for unit in [
    "challtestsrv",
    "pebble",
    "fake-cloudflare",
    "dns-worker",
    "dd-acme.socket",
]:
    box1.wait_for_unit(unit)
box1.wait_until_succeeds("curl -s -o /dev/null http://127.0.0.1:8787/", timeout=120)
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
    box1.succeed("systemctl start acme-order-renew-test.invalid.service")
    box1.wait_until_succeeds(
        "systemctl show -p Result acme-order-renew-test.invalid.service | grep -q success",
        timeout=180,
    )
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
