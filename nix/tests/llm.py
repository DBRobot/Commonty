box.wait_for_unit("dd-llm-key.service")
box.wait_for_unit("llama-swap.service")
box.wait_for_unit("sockets.target")

sock = "--unix-socket /run/dd-llm/llm.sock"
key = "-H 'Authorization: Bearer testkey'"

# the socket is nginx's group's, and nobody else's
box.succeed("stat -c '%a %G' /run/dd-llm/llm.sock | grep -qx '660 nginx'")

# without the key: refused
code = box.succeed(f"curl -s -o /dev/null -w '%{{http_code}}' {sock} http://x/v1/models").strip()
assert code == "401", code

# with it: the model list, named as the page shows it
models = box.succeed(f"curl -sf {sock} {key} http://x/v1/models")
assert '"tiny"' in models and "Tiny" in models, models

# a request wakes the model, and a real answer comes back
out = box.succeed(
    f"curl -sf -m 300 {sock} {key} -H 'content-type: application/json' "
    "-d '{\"model\":\"tiny\",\"prompt\":\"Once upon a time\",\"max_tokens\":8}' http://x/v1/completions"
)
assert '"choices"' in out, out
assert "tiny" in box.succeed(f"curl -sf {sock} {key} http://x/running")

# nothing else on the box reaches llama-swap or a model server: not its
# port, and not the model server's (llama-swap hands them out from 5800)
box.fail("curl -s -m 3 http://127.0.0.1:8081/v1/models")
box.fail("curl -s -m 3 http://127.0.0.1:5800/health")
box.fail("curl -s -m 3 http://127.0.0.1:5801/health")

# and nothing is kept of what was asked
box.fail("journalctl -u llama-swap --no-pager | grep -q 'Once upon a time'")

# Web search: SearXNG answers on loopback alone, in the json the gate reads,
# and may not reach inside the house or the tailnet. (No internet here, so
# the engines come back empty; the shape is what is checked.)
box.wait_for_unit("searx.service")
box.wait_until_succeeds("curl -sf 'http://127.0.0.1:8888/search?q=test&format=json' | grep -q '\"results\"'", timeout=120)
listen = box.succeed("ss -Hltn 'sport = :8888'")
assert "127.0.0.1:8888" in listen and "0.0.0.0" not in listen and "[::]" not in listen, listen
deny = box.succeed("systemctl show -p IPAddressDeny searx.service")
assert "192.168.0.0/16" in deny and "100.64.0.0/10" in deny, deny
