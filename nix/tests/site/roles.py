# Who gets which page and API (checks.site-roles): every kind of person
# against the gateway box's real nginx and gate (stack.py), one row a place,
# one column a person. The expected table is written from the docs, not
# from what the box does today:
#
#   docs/dev/gate.md      "Roles", "The answer to nginx", "Routes"
#   docs/dev/joining.md   "Access" (a box that counts is as good as an invite;
#                          a ban is a ban), "The box" (its Metrics)
#   docs/guide/guests.md  "What a guest can and cannot do"
#
# A cell is what the person must get:
#   Y  let in: 2xx, the page or the service behind it
#   W  the waiting page ("Nothing here is open to you yet"), 200
#   -  refused, signed in: 403, 401, or sent home / to the waiting page
#   L  refused, signed out: 401, 403, or sent to sign in; never content
#   X  the connection closed (444): a private name, asked from outside
#
# KNOWN lists the cells main gets wrong today, each with why; a cell there
# that starts to pass fails too, so the list is cut when a fix lands.
import re
import sys
import time

import people

COLS = ["anon", "gwen", "sam", "rex", "mia", "bob", "david"]
WHO = {"anon": None, **{p: p for p in people.PEOPLE}}
JB = people.BOBS_BOX

# host, path, method, expected (anon gwen sam rex mia bob david), why
TABLE = [
    # --- the gate's pages (gate.md "Routes"); a page is a shell that
    # fills itself from the APIs below, so "Y" on a page is its file only
    (
        "home",
        "/_dd/home",
        "GET",
        "L Y W W Y Y Y",
        "home, or the waiting page for anyone not let in",
    ),
    ("home", "/", "GET", "L Y W W Y Y Y", "home's / is the home page"),
    (
        "home",
        "/_dd/settings",
        "GET",
        "L Y Y Y Y Y Y",
        "Settings: the waiting page links its Leave tab",
    ),
    ("home", "/_dd/friends", "GET", "L Y - - Y Y Y", "Friends: member or guest"),
    (
        "home",
        "/_dd/files",
        "GET",
        "L - - - Y Y Y",
        "Files: a member's browser is handed to the app",
    ),
    ("home", "/_dd/media", "GET", "L - - - Y Y Y", "Movies & TV: as Files"),
    ("home", "/_dd/chat", "GET", "L - - - Y Y Y", "Chat: member (or the demo)"),
    (
        "home",
        "/_dd/adblock",
        "GET",
        "L - - - - - Y",
        "Ad blocking: the household (david) alone",
    ),
    (
        "home",
        "/_dd/git",
        "GET",
        "L Y Y Y Y Y Y",
        "Git's page: any session; the forge's API decides",
    ),
    (
        "home",
        "/_dd/metrics",
        "GET",
        "L - - - - Y Y",
        "Metrics: the admin, a box's owner, and a member their own joined box (joining.md 'The box')",
    ),
    (
        "home",
        "/_dd/photos",
        "GET",
        "Y Y Y Y Y Y Y",
        "Photos hands any browser to the app",
    ),
    ("home", "/_dd/login", "GET", "Y Y Y Y Y Y Y", "sign-in: anyone"),
    ("home", "/_dd/docs/index.json", "GET", "Y Y Y Y Y Y Y", "the docs: anyone"),
    # --- the gate's APIs
    ("home", "/_dd/me", "GET", "L Y - - Y Y Y", "who, role, menu: 403 without a role"),
    (
        "home",
        "/_dd/sitemap.json",
        "GET",
        "L - - - Y Y Y",
        "every place to open: member or the demo",
    ),
    ("home", "/_dd/fleet.json", "GET", "L - - - Y Y Y", "boxes' release: member"),
    ("home", "/_dd/house", "GET", "L - - - Y Y Y", "the house's boxes: member"),
    ("home", "/_dd/storage/mine", "GET", "L - - - Y Y Y", "allowance and use: member"),
    (
        "home",
        "/_dd/directory",
        "GET",
        "L - - - Y Y Y",
        "the directory's names: member, by invite or by box",
    ),
    (
        "home",
        "/_dd/directory/mia",
        "GET",
        "L - - - Y Y Y",
        "an entry: member, or the person",
    ),
    (
        "home",
        "/_dd/directory/gwen",
        "GET",
        "L Y - - Y Y Y",
        "an entry: the person themselves too",
    ),
    (
        "home",
        "/_dd/friends/list",
        "GET",
        "L Y - - Y Y Y",
        "friends and links: member or guest",
    ),
    ("home", "/_dd/signins", "GET", "L Y - - Y Y Y", "sessions: member or guest"),
    ("home", "/_dd/leave", "GET", "L Y Y Y Y Y Y", "where they stand: any session"),
    (
        "home",
        "/_dd/chat/search?q=otters",
        "GET",
        "L - - - Y Y Y",
        "Chat's web search: member",
    ),
    (
        "home",
        "/_dd/adblock/state",
        "GET",
        "L - - - - - Y",
        "ad blocking: the household",
    ),
    (
        "home",
        "/_dd/friends/link",
        "POST",
        "L - - - Y Y Y",
        "a friend link: members make them, guests do not",
    ),
    # --- the services' own names, behind auth_request (gate.md "The answer to nginx")
    ("files", "/_dd/files", "GET", "L - - - Y Y Y", "Files"),
    ("games", "/", "GET", "L Y - - Y Y Y", "Games: members, and a guest (games alone)"),
    (
        "games",
        "/api/servers",
        "GET",
        "L Y - - Y Y Y",
        "the games manager's API, as its page asks it",
    ),
    ("git", "/", "GET", "L Y Y Y Y Y Y", "Git's page: any session"),
    (
        "git",
        "/api/v1/repos/search",
        "GET",
        "Y - - - Y Y Y",
        "the forge's API: nobody goes through as anonymous (public repos, gate.md: a 401 lets the forge decide); a signed-in non-member is refused",
    ),
    (
        "git",
        "/david/commonty/raw/branch/main/README.md",
        "GET",
        "Y - - - Y Y Y",
        "a repository's files: as its API",
    ),
    ("llm", "/", "GET", "L - - - Y Y Y", "Chat's model: members"),
    ("llm", "/v1/models", "GET", "L - - - Y Y Y", "the model list: members"),
    (
        "metrics",
        "/",
        "GET",
        "L - - - - Y Y",
        "the Metrics page: the admin; a member with a joined box sees it (joining.md)",
    ),
    (
        "metrics",
        "/api/v1/query?query=up",
        "GET",
        "L - - - - - Y",
        "the fleet's figures: the admin and the boxes' owner",
    ),
    (
        "metrics",
        f"/joined/{JB}/api/v1/query?query=up",
        "GET",
        "L - - - - Y Y",
        "a joined box's figures: its member and the admin",
    ),
    (
        "photos",
        "/",
        "GET",
        "Y Y Y Y Y Y Y",
        "Photos' page hands any browser to the app",
    ),
]

# from outside (the tunnel), signed out: private names close, public ones answer
OUTSIDE = [
    ("home", "/_dd/home", "L", "home is public: sign-in"),
    ("@", "/", "Y", "the front page"),
    ("@", "/docs", "Y", "the docs"),
    ("vault", "/", "Y", "Passwords' public door (Vaultwarden signs in)"),
    ("code", "/", "Y", "the forge's public door"),
    ("files", "/_dd/files", "X", "private"),
    ("games", "/", "X", "private"),
    ("git", "/", "X", "private"),
    (
        "git",
        "/api/v1/repos/search",
        "X",
        "private: the forge's anonymous reads stay on the fleet network",
    ),
    ("llm", "/v1/models", "X", "private"),
    ("metrics", "/", "X", "private"),
    ("photos", "/", "X", "private"),
]

# main today, cell by cell: (host, path, person) -> why it is wrong there
KNOWN = {
    (
        "home",
        "/_dd/metrics",
        "bob",
    ): "the Metrics page is refused to a member whose only box is a joined one (metrics_page: sees_box/boxes_seen read the release's boxes alone); joining.md says it shows them their own",
    (
        "metrics",
        "/",
        "bob",
    ): "the same: verify() lets a member onto metrics.<domain> only if sees_box, so the page never loads for them, though their /joined/<box>/ query is let through",
    (
        "home",
        "/_dd/directory",
        "bob",
    ): "Directory::member judges by the member list alone (replay), not the agreed access a box gives: a box-earned member is refused the directory in agreed mode",
    (
        "home",
        "/_dd/directory/mia",
        "bob",
    ): "the same (Directory::may_read -> Directory::member)",
    ("home", "/_dd/directory/gwen", "bob"): "the same",
}


def outcome(r, signed_in):
    loc = r.header("location") or ""
    if r.status == 444:
        return "X"
    if r.status in (301, 302, 303, 307, 308):
        if "/_dd/login" in loc:
            return "L" if not signed_in else "-"
        # sent home: refused (403 -> @waiting), or a page that moved
        return "-"
    if r.status in (401, 403):
        return "-" if signed_in else "L"
    if 200 <= r.status < 300:
        if b"Nothing here is open to you yet" in r.body:
            return "W"
        return "Y"
    return str(r.status)


def follow(s, host, path, who, method):
    """a redirect within the same name to a path of the gate's (files' / to
    /_dd/files) is followed once, as a browser would"""
    r = s.get(
        host,
        path,
        who=who,
        method=method,
        body=b"{}" if method == "POST" else None,
        headers={
            "content-type": "application/json",
            "origin": "https://" + s.name(host),
        }
        if method == "POST"
        else None,
    )
    loc = r.header("location") or ""
    m = re.match(rf"https://{re.escape(s.name(host))}(/[^?]*)$", loc)
    if r.status in (301, 302) and m and "/_dd/login" not in loc and m.group(1) != path:
        r = s.get(host, m.group(1), who=who)
    return r


def run(s):
    bad, fixed, n = [], [], 0
    t0 = time.monotonic()
    for host, path, method, want, why in TABLE:
        cells = want.split()
        assert len(cells) == len(COLS), (host, path)
        for col, w in zip(COLS, cells):
            who = WHO[col]
            got = outcome(follow(s, host, path, who, method), who is not None)
            n += 1
            known = KNOWN.get((host, path, col))
            if got != w and not known:
                bad.append(
                    f"{method} {s.name(host)}{path} as {col}: got {got}, want {w} ({why})"
                )
            elif got == w and known:
                fixed.append(
                    f"{method} {s.name(host)}{path} as {col}: now right; take it off KNOWN ({known})"
                )
    for host, path, w, why in OUTSIDE:
        got = outcome(s.get(host, path, outside=True), False)
        n += 1
        if got != w:
            bad.append(
                f"GET {s.name(host)}{path} from outside: got {got}, want {w} ({why})"
            )
    print(
        f"site-roles: {n} cells in {time.monotonic() - t0:.1f}s, {len(KNOWN)} known wrong on main"
    )
    for k, v in KNOWN.items():
        print(f"  known: {k[0]}{k[1]} as {k[2]}: {v}")
    for b in bad + fixed:
        print("FAIL", b)
    return not bad and not fixed


if __name__ == "__main__":
    from stack import Stack

    out, conf, nginx, verify, env = sys.argv[1:6]
    with Stack(out + "/stack", conf, nginx, verify, env) as s:
        ok = run(s)
    sys.exit(0 if ok else 1)
