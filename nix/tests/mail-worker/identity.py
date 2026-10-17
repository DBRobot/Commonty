# The mail Worker reading who is a member from the agreed identity state
# (docs/dev/identity-state.md) instead of judging it: one instance in shadow
# (judges, reads, says where they differ, goes by its own) and one in agreed
# (goes by the agreed state alone), against fake.py's /identity. Run by
# mail-worker.nix after flow.py.
import json, sys, urllib.error, urllib.request

F = "http://127.0.0.1:8899"
SHADOW, AGREED = "http://localhost:8791", "http://localhost:8790"
shadow_log = sys.argv[1]
failed = []


def ok(cond, what):
    print(("ok   " if cond else "FAIL ") + what, flush=True)
    if not cond:
        failed.append(what)


def post(url, body, origin=None):
    h = {"content-type": "application/json"}
    if origin:
        h["origin"] = origin
    req = urllib.request.Request(
        url, data=json.dumps(body).encode(), method="POST", headers=h
    )
    try:
        r = urllib.request.urlopen(req)
        return r.status, json.load(r)
    except urllib.error.HTTPError as e:
        t = e.read()
        try:
            return e.code, json.loads(t)
        except Exception:
            return e.code, {"raw": t[:120]}


def agreed(**v):
    post(F + "/identity-set", v)


def challenge(w):
    return post(w + "/api/challenge", {"name": "tester"}, origin=w)


def sign(c):
    return post(
        F + "/sign",
        {
            "challenge": c,
            "origin": "https://mail.commonty.test",
            "rp": "commonty.test",
            "flags": 5,
        },
    )[1]


def said(what):
    with open(shadow_log) as f:
        return any(
            "identity-shadow" in l and f'"{what}"' in l and '"tester"' in l for l in f
        )


# shadow: the two agree, nothing said
agreed(status="member", entry="v3", left=0)
s, c = challenge(SHADOW)
ok(s == 200 and c.get("allow") == ["cGsx"], "shadow: a member signs in as before")
ok(not said("member"), "shadow: the agreed state agrees, nothing said")
# the agreed state says otherwise: still its own word, and the difference said
agreed(status="none")
s, c = challenge(SHADOW)
ok(s == 200, "shadow: goes by its own judgment")
ok(said("member"), "shadow: says where the agreed state differs")

# agreed: the passkeys the agreed entry has, not the directory's
agreed(status="member", entry="v5")
s, c = challenge(AGREED)
ok(s == 200 and c.get("allow") == ["cGsy"], "agreed: the agreed entry's passkeys")
s, r = post(
    AGREED + "/api/login",
    {"token": c["token"], "id": "cGsy", "assertion": sign(c["challenge"])},
    origin=AGREED,
)
ok(s == 200 and r.get("ok"), "agreed: signs in with the agreed entry's passkey")
s, c = challenge(AGREED)
s, r = post(
    AGREED + "/api/login",
    {"token": c["token"], "id": "cGsx", "assertion": sign(c["challenge"])},
    origin=AGREED,
)
ok(s == 403, "agreed: a passkey the agreed entry no longer has: refused")
# not a member by the agreement: nothing
for status in ("none", "suspended", "revoked", "left"):
    agreed(status=status)
    ok(challenge(AGREED)[0] == 403, f"agreed: {status}: no sign-in")

# leaving, by the agreement: a member's address stays; their own leaving
# takes it, and nothing of the release's removal is needed
agreed(status="member", entry="v3")
challenge(AGREED)
ok(
    post(AGREED + "/api/forget", {"name": "tester"}, origin=AGREED)[0] == 403,
    "agreed: a member's address is not forgotten",
)
agreed(status="left", left=2_000_000_000_000)
ok(
    post(AGREED + "/api/forget", {"name": "tester"}, origin=AGREED)[0] == 403,
    "agreed: leaving in the future: not yet",
)
agreed(status="left", left=1_000_000_000_000)
s, r = post(AGREED + "/api/forget", {"name": "tester"}, origin=AGREED)
ok(s == 200 and r.get("gone"), "agreed: left: their address goes")

if failed:
    print(f"{len(failed)} failed", flush=True)
    sys.exit(1)
