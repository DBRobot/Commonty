# The people the site tests sign in as, one of each kind the gate tells
# apart (docs/dev/gate.md "Roles", docs/dev/joining.md "Access",
# docs/guide/guests.md), as the agreed identity state has them:
#
#   david  the fleet's admin and both boxes' owner; in by invite
#   mia    a member by invite (the release's member list)
#   bob    a member by a box of his that counts (access "box")
#   gwen   mia's guest: came in by her friend link, not a member
#   sam    signed up and not let in (access "none")
#   rex    removed from the member list: a ban is a ban
#
# Each is signed in with this box's own session cookie, made with a session
# secret the test gives the gate (session.rs): the cookie a passkey sign-in
# sets, without the passkey. Signing in itself is the sso check's.
import base64
import copy
import hashlib
import hmac
import json
import os
import time

ADMIN = "david"
# bob's box, joined and enlisted: a joined box's name is j and ten hex
BOBS_BOX = "j0123456789"
PEOPLE = ["david", "mia", "bob", "gwen", "sam", "rex"]
SECRET = bytes(range(32))
_tester = os.environ.get("SITE_TESTER")


def _root(name):
    # a made-up ed25519 public key per person: only its id is ever compared
    return base64.b64encode(
        hashlib.sha256(b"site-root " + name.encode()).digest()
    ).decode()


def member_id(name):
    return hashlib.sha256(_root(name).encode()).hexdigest()


def entry(name):
    e = copy.deepcopy(json.load(open(_tester))["v5"])
    e["entry"]["name"] = name
    e["entry"]["root"] = _root(name)
    e["entry"]["devices"] = [
        {"added": 1, "fingerprint": "dev-" + name, "public_key": _root(name)}
    ]
    return e


def member_list():
    return {
        "members": [member_id("david"), member_id("mia")],
        "revoked": [member_id("rex")],
    }


def agreed_state():
    def one(name, status, access, boxes=()):
        return {
            "id": member_id(name),
            "ids": [member_id(name)],
            "status": status,
            "access": access,
            "boxes": list(boxes),
            "item": entry(name),
            "enlisted": [{"box": b, "counts": True} for b in boxes],
        }

    now = int(time.time())
    return {
        "height": 1,
        "members": {
            "david": one("david", "member", "invite"),
            "mia": one("mia", "member", "invite"),
            "bob": one("bob", "member", "box", [BOBS_BOX]),
            "gwen": one("gwen", "none", "none"),
            "sam": one("sam", "none", "none"),
            "rex": one("rex", "revoked", "none"),
        },
        "outcomes": {},
        "enlisted": {BOBS_BOX: {"ticket": "0" * 32, "owner": "bob", "since": now}},
        "waiting": {},
    }


def control_state():
    """dd-control's own state, as /state answers it: the boxes that joined"""
    return {"joined": {BOBS_BOX: {"at": 1, "key": "site-test-key"}}}


def write_gate_state(gate_dir, domain):
    """what the gate keeps on its own disk: the session secret, and who is
    whose friend (friends.json is this box's alone, never agreed)"""
    os.makedirs(gate_dir + "/keys", exist_ok=True)
    os.chmod(gate_dir, 0o700)
    for name, data in (
        ("session.secret", SECRET),
        (
            "friends.json",
            json.dumps(
                {
                    "links": {},
                    "pairs": [{"a": "mia", "b": "gwen", "since": 1}],
                    "guests": {"gwen": {"by": "mia", "since": 1}},
                }
            ).encode(),
        ),
    ):
        p = f"{gate_dir}/{name}"
        with open(os.open(p, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "wb") as f:
            f.write(data)


def cookie(gate_dir, who, ttl=3600):
    """dd_session=<who>~<id>.<exp>.<mac>, as session.rs issues it"""
    exp = int(time.time()) + ttl
    sid = (
        base64.urlsafe_b64encode(hashlib.sha256(f"{who}{exp}".encode()).digest()[:9])
        .rstrip(b"=")
        .decode()
    )
    w = f"{who}~{sid}"
    mac = hmac.new(SECRET, f"{w}.{exp}".encode(), hashlib.sha256).digest()
    return (
        f"dd_session={w}.{exp}." + base64.urlsafe_b64encode(mac).rstrip(b"=").decode()
    )
