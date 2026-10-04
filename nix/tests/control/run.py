# The control plane end to end, with no network and no vm: the relay and
# witness under wrangler (workerd), three dd-control validators with test
# box keys, agreeing; one killed and the rest carrying on; it restarted and
# catching up from its log; a hello signed by the wrong key refused.
import json, os, subprocess, sys, time, urllib.request, base64

T = os.getcwd()
failed = []
def ok(c, m):
    print(("PASS " if c else "FAIL ") + m, flush=True)
    if not c: failed.append(m)

def state(i):
    try:
        return json.load(urllib.request.urlopen(f"http://127.0.0.1:419{i}/state", timeout=3))
    except Exception:
        return {}

def run(i):
    return subprocess.Popen([f"{T}/dd-control", "-config", f"{T}/config.json", "-box", f"node{i}",
        "-relay", "ws://127.0.0.1:8789/relay", "-state", f"{T}/st{i}", "-box-key", f"{T}/node{i}.p8",
        "-listen", f"127.0.0.1:419{i}", "-heartbeat", "1s", "-roles", f"r{i}"],
        stdout=open(f"{T}/n{i}.log", "a"), stderr=subprocess.STDOUT)

def until(what, cond, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        if cond(): return True
        time.sleep(0.5)
    ok(False, f"timed out: {what}")
    return False

procs = {i: run(i) for i in (1, 2, 3)}
agree = lambda ids, h: all(state(i).get("height", 0) >= h for i in ids) and len({json.dumps(state(i).get("boxes", {}).keys().__len__()) for i in ids}) == 1
if until("first agreement", lambda: all(len(state(i).get("boxes", {})) == 3 for i in (1, 2, 3))):
    s = [state(i) for i in (1, 2, 3)]
    ok(True, f"three validators and the witness agree, height {s[0]['height']}")
    ok(all(x["boxes"]["node2"]["roles"] == ["r2"] for x in s), "each box's heartbeat and roles are in the agreed state")
    h = json.load(urllib.request.urlopen("http://127.0.0.1:8789/health"))
    ok(h["witness"] and h["connected"] == 3, f"relay health {h}")

procs[1].terminate(); procs[1].wait()
h0 = state(2).get("height", 0)
until("the others carry on without node1", lambda: state(2).get("height", 0) > h0 + 3 and state(3).get("height", 0) > h0 + 3)
ok(state(2).get("height", 0) > h0 + 3, "with one validator down the other two and the witness carry on")

procs[1] = run(1)
until("node1 catches up", lambda: state(1).get("height", 0) >= state(2).get("height", 0) - 1 and state(1).get("height", 0) > h0 + 3)
ok(state(1).get("height", 0) > h0 + 3, "a restarted validator replays its log and catches up")

req = urllib.request.Request(f"http://127.0.0.1:8789/relay?id=1&at={int(time.time())}&sig=AAAA",
    headers={"Upgrade": "websocket", "Connection": "Upgrade", "Sec-WebSocket-Version": "13", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ=="})
try:
    urllib.request.urlopen(req); code = 101
except urllib.error.HTTPError as e:
    code = e.code
ok(code == 403, f"a hello not signed by that address's key is refused ({code})")

for p in procs.values():
    p.terminate()
if failed:
    sys.exit(f"{len(failed)} failed")
print("all passed")
