start_all()
for m in (a, b):
    m.wait_for_unit("dd-store-setup.service")
    m.wait_for_unit("nginx.service")

nixc = "nix --extra-experimental-features nix-command"
url = b.succeed("cat /etc/cache-url").strip()
env = b.succeed("cat /etc/cache-env").strip()
# b reaches the cache at a's door, not a store of its own
assert "store.a.test.invalid:3443" in url, url

# b's key to a's store, which dd-store-at hands it
b.wait_until_succeeds(
    f"systemctl restart dd-store-at.service && test -s {env}", timeout=120
)
a.succeed(
    f"systemctl restart dd-store-at.service && test -s {a.succeed('cat /etc/cache-env').strip()}"
)

b.succeed("nix-store --generate-binary-cache-key b-1 /root/cache.sec /root/cache.pub")
path = b.succeed("echo 'built on b' > /root/f && nix-store --add /root/f").strip()

with subtest("b pushes through a's door, signed, over TLS"):
    b.succeed(
        f"set -a; . {env}; set +a; "
        f"{nixc} copy --to '{url}&secret-key=/root/cache.sec' {path}"
    )

with subtest("and reads it back there"):
    b.succeed(
        f"set -a; . {env}; set +a; "
        f"{nixc} path-info --option narinfo-cache-negative-ttl 0 --store '{url}' {path}"
    )

with subtest("a keeps it, in its own store"):
    a_url = a.succeed("cat /etc/cache-url").strip()
    a_env = a.succeed("cat /etc/cache-env").strip()
    assert "127.0.0.1" in a_url, a_url
    out = a.succeed(
        f"set -a; . {a_env}; set +a; {nixc} path-info --sigs --store '{a_url}' {path}"
    )
    assert "b-1:" in out, out
