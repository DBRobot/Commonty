queue = "/var/lib/dd-cache-upload/queue"
unit = "dd-cache-upload.service"
cache = nix["cache"]

box.wait_for_unit("multi-user.target")
box.succeed(
    "nix-store --generate-binary-cache-key test-1 /var/lib/test-cache.sec /var/lib/test-cache.pub"
)
box.succeed(
    "cat > /root/t.nix <<'EOF'\n"
    '{ n, big ? false, ref ? "" }:\n'
    'let sh = builtins.storePath (dirOf (dirOf "' + nix["sh"] + '")); in\n'
    "derivation {\n"
    "  name = n;\n"
    "  system = builtins.currentSystem;\n"
    '  builder = "${sh}/bin/sh";\n'
    '  ref = if ref == "" then "" else builtins.storePath ref;\n'
    '  args = [ "-c" (if big\n'
    '    then "${sh}/bin/mkdir $out && ${sh}/bin/head -c 314572800 /dev/zero > $out/big"\n'
    '    else "echo ${n} $ref > $out") ];\n'
    "}\n"
    "EOF"
)


def build(name, *args):
    return box.succeed(
        f"nix-build --no-out-link /root/t.nix --argstr n {name} {' '.join(args)}"
    ).strip()


def in_cache(path):
    return (
        box.execute(
            f"nix --extra-experimental-features nix-command path-info --store '{cache}' {path}"
        )[0]
        == 0
    )


def drained():
    box.wait_until_succeeds(
        f'[ -z "$(ls -A {queue})" ] && ! systemctl is-active -q {unit}', timeout=600
    )


def starts():
    return int(
        box.succeed(f"journalctl -u {unit} -o cat | grep -c '^Started '").strip()
    )


def healthy():
    result = box.succeed(f"systemctl show -P Result {unit}").strip()
    assert result == "success", f"the uploader's result is {result}"
    box.succeed("systemctl is-active -q dd-cache-queue.path")


assert box.succeed(f"systemctl show -P StartLimitIntervalUSec {unit}").strip() == "0"

with subtest("builds in rounds start it again and again; it never stops for good"):
    built = []
    for r in range(8):
        for i in range(3):
            built.append(build(f"r{r}-{i}"))
        drained()
    healthy()
    # systemd refused a sixth start in ten seconds
    assert starts() >= 8, f"only {starts()} starts"
    missing = [p for p in built if not in_cache(p)]
    assert not missing, f"not in the cache: {missing}"

with subtest("a big path goes with a closure, not on its own, past the base timeout"):
    big = build("big", "--arg big true")
    drained()
    assert not in_cache(big), "a 300 MiB path went on its own account"
    top = build("top", "--argstr ref", big)
    drained()
    healthy()
    assert in_cache(top) and in_cache(big), "the closure did not reach the cache"

with subtest("a cache that refuses keeps the queue, then takes it"):
    box.succeed(
        "mv /var/lib/test-cache /var/lib/test-cache.away && touch /var/lib/test-cache"
    )
    held = [build(f"held-{i}") for i in range(3)]
    box.wait_until_succeeds(
        f"[ $(ls {queue} | grep -c '\\.try[0-9]') -gt 0 ]", timeout=120
    )
    box.wait_until_succeeds(
        f"[ $(journalctl -u {unit} -o cat | grep -c 'tried again later') -gt 0 ]",
        timeout=60,
    )
    box.succeed(f"systemctl is-active -q {unit}")
    assert not any(in_cache(p) for p in held)
    box.succeed(
        "rm /var/lib/test-cache && mv /var/lib/test-cache.away /var/lib/test-cache"
    )
    drained()
    healthy()
    missing = [p for p in held if not in_cache(p)]
    assert not missing, f"lost when the cache refused: {missing}"
