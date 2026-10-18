# The mail Worker end to end, in a browser: change an email through the
# three pages, the line in Settings, and the refusals at its API. Run by
# mail-worker.nix against the real Worker with a fake Cloudflare (fake.py).
import base64, hashlib, json, sys, urllib.request
from playwright.sync_api import sync_playwright

W = "http://localhost:8787"
F = "http://127.0.0.1:8899"
b64u = lambda b: base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def sign(challenge, origin="https://mail.commonty.test", rp="commonty.test", flags=5):
    req = urllib.request.Request(
        F + "/sign",
        data=json.dumps(
            {"challenge": challenge, "origin": origin, "rp": rp, "flags": flags}
        ).encode(),
        method="POST",
        headers={"content-type": "application/json"},
    )
    return json.load(urllib.request.urlopen(req))


def fake(path, body=None):
    req = urllib.request.Request(
        F + path,
        data=json.dumps(body).encode() if body is not None else None,
        method="POST" if body is not None else "GET",
        headers={"content-type": "application/json"},
    )
    return json.load(urllib.request.urlopen(req))


STUB = """
// the browser's passkey prompt, answered by the made-up member's key
if (!navigator.credentials) Object.defineProperty(navigator, 'credentials', { value: {}, configurable: true });
Object.defineProperty(navigator.credentials, 'get', { configurable: true, value: async (o) => {
  const c = btoa(String.fromCharCode(...new Uint8Array(o.publicKey.challenge))).replace(/\\+/g,'-').replace(/\\//g,'_').replace(/=+$/,'');
  const a = await window.__sign(c);
  const u = (s) => Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')), x => x.charCodeAt(0)).buffer;
  return { rawId: u(a.id), response: { authenticatorData: u(a.authenticatorData), clientDataJSON: u(a.clientDataJSON), signature: u(a.signature) } };
} });
"""


def settled(page):
    # the email line has its answer; asked from here, as the page's own
    # policy refuses the string a wait_for_function would evaluate
    import time

    for _ in range(100):
        if page.inner_text("#status") != "Checking…":
            return
        time.sleep(0.1)
    raise TimeoutError("the email line never settled")


failed = []


def ok(c, m):
    print(("PASS " if c else "FAIL ") + m, flush=True)
    if not c:
        failed.append(m)


with sync_playwright() as p:
    b = p.chromium.launch(
        headless=True,
        args=[
            "--mute-audio",
            "--no-sandbox",
            "--disable-dev-shm-usage",
            "--disable-gpu",
        ],
    )
    ctx = b.new_context(viewport={"width": 1000, "height": 760})
    ctx.expose_function("__sign", lambda c: sign(c))
    ctx.add_init_script(STUB)
    ctx.route(
        "https://files.commonty.test/**",
        lambda r: r.fulfill(
            status=200, body="<p>settings</p>", content_type="text/html"
        ),
    )
    pg = ctx.new_page()
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
    back = "https://files.commonty.test/_dd/settings"
    pg.goto(f"{W}/change?name=tester&back={back}")
    pg.click("#pass")
    pg.wait_for_selector("#s2:not([hidden])", timeout=10000)
    ok(True, "passkey accepted, on to the address")
    pg.fill("#email", "first@example.com")
    pg.click("#save")
    pg.wait_for_selector("#s3:not([hidden])", timeout=10000)
    ok(
        pg.inner_text("#sent") == "first@example.com",
        "check-your-email shows the address",
    )
    st = fake("/state")
    ok("first@example.com" in st["sent"], "a confirmation went out")
    ok(
        any(
            r["actions"][0]["value"] == ["first@example.com"]
            and r["matchers"][0]["value"] == "tester@commonty.test"
            for r in st["rules"].values()
        ),
        "rule forwards tester@ to it",
    )
    fake("/verify", {"email": "first@example.com"})
    pg.wait_for_url("**/_dd/settings?email=changed", timeout=15000)
    ok(True, "moved on by itself once opened -> " + pg.url)

    # the aliases, kept in step by that same look: security@ to the confirmed
    # address; the dropped one gone; a member's own rule and refused aliases untouched
    def rules_to(addr):
        return [
            r
            for r in fake("/state")["rules"].values()
            if r["matchers"][0]["value"] == addr
        ]

    sec = rules_to("security@commonty.test")
    ok(
        len(sec) == 1
        and sec[0]["actions"][0]["value"] == ["first@example.com"]
        and sec[0]["name"] == "Commonty alias security",
        f"security@ forwards to the confirmed address {sec}",
    )
    ok(not rules_to("old@commonty.test"), "a dropped alias loses its rule")
    other = rules_to("other@commonty.test")
    ok(
        len(other) == 1
        and other[0]["actions"][0]["value"] == ["other@example.com"]
        and other[0]["name"] == "Commonty member other",
        "an alias that is a member's address already is refused, and the member's rule untouched",
    )
    ok(not rules_to("Bad.One@commonty.test"), "an alias that is no alias is refused")
    # the email line, signed in here already
    pg.goto(f"{W}/row?name=tester&back={back}")
    settled(pg)
    ok(
        pg.inner_text("#addr") == "first@example.com"
        and pg.inner_text("#status") == "Confirmed",
        "email line shows address: " + pg.inner_text(".er"),
    )
    # a stranger's browser: no address until the passkey
    ctx2 = b.new_context()
    ctx2.expose_function("__sign", lambda c: sign(c))
    ctx2.add_init_script(STUB)
    p2 = ctx2.new_page()
    p2.goto(f"{W}/row?name=tester&back={back}")
    settled(p2)
    ok(
        p2.is_hidden("#addr")
        and p2.is_visible("#show")
        and p2.inner_text("#status") == "Confirmed",
        "without passkey: status only, Show button",
    )
    p2.click("#show")
    p2.wait_for_selector("#addr:not([hidden])", timeout=10000)
    ok(p2.inner_text("#addr") == "first@example.com", "Show reveals after passkey")
    # change to an address already confirmed elsewhere in the account: no wait
    fake("/verify", {"email": "first@example.com"})
    pg.goto(f"{W}/change?name=tester&back={back}")
    pg.wait_for_selector("#s2:not([hidden])", timeout=10000)
    ok(True, "signed in a moment ago: straight to the address")
    pg.fill("#email", "second@example.com")
    pg.click("#save")
    pg.wait_for_selector("#s3:not([hidden])")
    st = fake("/state")
    ok(
        "first@example.com" not in st["addrs"],
        "the old address is removed from the account",
    )
    n0 = fake("/state")["sent"].count("second@example.com")
    pg.click("#again")
    pg.wait_for_timeout(800)
    ok(
        fake("/state")["sent"].count("second@example.com") == n0 + 1,
        "send it again sends again",
    )
    ok(not errs, f"no errors on the pages {errs}")
    # the API refuses what it should
    import urllib.error

    def call(path, body=None, origin="http://localhost:8787", cookie=None):
        h = {"content-type": "application/json", "origin": origin}
        if cookie:
            h["cookie"] = cookie
        req = urllib.request.Request(
            W + path,
            data=json.dumps(body).encode() if body is not None else None,
            method="POST" if body is not None else "GET",
            headers=h,
        )
        try:
            r = urllib.request.urlopen(req)
            return r.status, json.load(r), r.headers.get("set-cookie")
        except urllib.error.HTTPError as e:
            t = e.read()
            try:
                return e.code, json.loads(t), None
            except Exception:
                return e.code, {"raw": t[:120]}, None

    ok(call("/api/me")[0] == 401, "no session: no address")
    ok(
        call("/api/email", {"email": "x@example.com"})[0] == 401,
        "no session: no change",
    )
    r = call("/api/challenge", {"name": "tester"}, origin="http://127.0.0.1:8787")
    ok(r[0] == 403, f"another origin cannot call it {r[:2]}")
    r = call("/api/challenge", {"name": "nobody"})
    ok(r[0] == 403, f"not a member: refused {r[:2]}")
    s, c, _ = call("/api/challenge", {"name": "tester"})
    ok(
        call(
            "/api/login", {"token": c["token"], "id": "cGsx", "assertion": sign("AAAA")}
        )[0]
        == 403,
        "answer to another challenge: refused",
    )
    ok(
        call(
            "/api/login",
            {
                "token": c["token"],
                "id": "cGsx",
                "assertion": sign(
                    c["challenge"], origin="https://evil.example", rp="evil.example"
                ),
            },
        )[0]
        == 403,
        "made for another site: refused",
    )
    ok(
        call(
            "/api/login",
            {
                "token": c["token"],
                "id": "cGsx",
                "assertion": sign(c["challenge"], flags=4),
            },
        )[0]
        == 403,
        "nobody present: refused",
    )
    ok(
        call(
            "/api/login",
            {
                "token": c["token"],
                "id": "cGsx",
                "assertion": sign(c["challenge"], origin="https://home.commonty.test"),
            },
        )[0]
        == 403,
        "made on a page a box serves: refused",
    )
    ok(
        call(
            "/api/login",
            {
                "token": c["token"] + "x",
                "id": "cGsx",
                "assertion": sign(c["challenge"]),
            },
        )[0]
        == 403,
        "tampered challenge token: refused",
    )
    s, _, ck = call(
        "/api/login",
        {"token": c["token"], "id": "cGsx", "assertion": sign(c["challenge"])},
    )
    ok(s == 200 and ck, "a right answer signs in")
    sess = ck.split(";")[0]
    ok(
        call("/api/me", cookie=sess)[1].get("email") == "second@example.com",
        "session reads the address",
    )
    # second@ is not confirmed yet: security@ forwards nowhere rather than to the old one
    ok(not rules_to("security@commonty.test"), "an unconfirmed address takes no alias")
    fake("/verify", {"email": "second@example.com"})
    call("/api/me", cookie=sess)
    sec = rules_to("security@commonty.test")
    ok(
        len(sec) == 1 and sec[0]["actions"][0]["value"] == ["second@example.com"],
        f"confirmed, security@ follows it {sec}",
    )

    # A member's own aliases (aliases.js): made, sent on, turned off, deleted
    import re, time

    def mail(to, sender="shop@example.org", head="", body="hello\r\n", raw=None):
        raw = (
            raw
            or (
                f"From: {sender}\r\nTo: {to}\r\nSubject: hi\r\n"
                f"Message-ID: <{time.time_ns()}@example.org>\r\n{head}\r\n{body}"
            ).encode()
        )
        req = urllib.request.Request(
            f"{W}/cdn-cgi/handler/email?from=shop@example.org&to={to}",
            data=raw,
            method="POST",
        )
        try:
            return urllib.request.urlopen(req).status
        except urllib.error.HTTPError as e:
            return e.code

    def forwarded():
        time.sleep(0.5)
        return open("wrangler.log").read().count("rcptTo: second@example.com")

    ok(call("/api/aliases")[0] == 401, "no session: no aliases")
    s, v, _ = call("/api/aliases", cookie=sess)
    ok(
        s == 200 and v["aliases"] == [] and v["max"] == 100,
        f"none yet, at most 100 {v}",
    )
    ca = fake("/state")["catch_all"]
    ok(
        ca["enabled"] and ca["actions"] == [{"type": "worker", "value": ["mail-e2e"]}],
        f"the zone's catch-all sends to this Worker {ca}",
    )
    s, nxt, _ = call("/api/aliases/next", {}, cookie=sess)
    addr = nxt.get("address", "")
    offer = {"n": nxt.get("n"), "day": nxt.get("day")}
    ok(
        call("/api/aliases/next", {"n": offer["n"]}, cookie=sess)[1].get("address")
        == addr,
        "asked again, the same address: Another cannot be pressed until one you like comes up",
    )
    ok(
        s == 200
        and re.fullmatch(r"[a-z]{3,10}\.[a-z]{3,10}\.[0-9]{4}@commonty\.test", addr),
        f"an address is offered {addr}",
    )
    ok(
        call(
            "/api/aliases",
            {**offer, "label": "Shop"},
            origin="http://127.0.0.1:8787",
            cookie=sess,
        )[0]
        == 403,
        "made from another origin: refused",
    )
    ok(
        call(
            "/api/aliases",
            {"address": "billing.paypal.0001@commonty.test", "label": "x"},
            cookie=sess,
        )[0]
        == 403,
        "an address of your own choosing, never offered: refused",
    )
    ok(
        call("/api/aliases", {"n": 99, "day": offer["day"], "label": "x"}, cookie=sess)[
            0
        ]
        == 403,
        "an offer past today's ten: refused",
    )
    s, v, _ = call("/api/aliases", {**offer, "label": "Shop"}, cookie=sess)
    ok(s == 200 and v["alias"]["address"] == addr and v["alias"]["on"], f"made {v}")
    ok(
        call("/api/aliases", {**offer, "label": "again"}, cookie=sess)[0] == 409,
        "the same address twice: refused",
    )
    n0 = forwarded()
    ok(
        mail(addr) == 200 and forwarded() == n0 + 1,
        "mail to it goes on to the confirmed address",
    )
    ok(mail("nobody.here.0000@commonty.test") == 400, "an address no one holds bounces")
    ok(mail("someone@commonty.test") == 400, "a name with no rule and no alias bounces")
    local = addr.split("@")[0]
    s, v, _ = call(f"/api/aliases/{local}", {"on": False}, cookie=sess)
    ok(s == 200 and not v["alias"]["on"] and v["alias"]["off"], "turned off")
    ok(mail(addr) == 400, "turned off, it bounces")
    call(f"/api/aliases/{local}", {"on": True, "label": "Hardware shop"}, cookie=sess)
    ok(mail(addr) == 200, "on again, it is sent on")
    ok(
        call("/api/aliases", cookie=sess)[1]["aliases"][0]["label"] == "Hardware shop",
        "renamed",
    )
    # deleting wants a passkey answer made for it, spent once
    ok(
        call(f"/api/aliases/{local}/delete", {}, cookie=sess)[0] == 403,
        "deleting with the session alone: refused",
    )

    def confirmed():
        c = call("/api/challenge", {"name": "tester"})[1]
        return {"token": c["token"], "id": "cGsx", "assertion": sign(c["challenge"])}

    yes = confirmed()
    ok(call(f"/api/aliases/{local}/delete", yes, cookie=sess)[0] == 200, "deleted")
    ok(mail(addr) == 400, "deleted, it bounces")
    ok(
        call(f"/api/aliases/{local}/delete", yes, cookie=sess)[0] in (403, 404),
        "the same passkey answer, again: refused",
    )
    ok(
        call(f"/api/aliases/{local}", {"on": True}, cookie=sess)[0] == 404
        and mail(addr) == 400,
        "a deleted alias cannot be turned back on",
    )
    ok(call("/api/aliases", cookie=sess)[1]["aliases"] == [], "and is off the list")
    ok(
        call("/api/aliases", {**offer, "label": "back"}, cookie=sess)[0] == 409,
        "a deleted address is never given out again",
    )

    # the page in Settings' frame: New alias, its dialog, the switch, the menu
    pg.goto(f"{W}/aliases?name=tester&back={back}")
    pg.wait_for_selector("#new:not([disabled])", timeout=10000)
    ok("No aliases yet" in pg.inner_text("#state"), "the page says there are none")
    pg.click("#new")
    for _ in range(50):
        if "@" in pg.inner_text("#next"):
            break
        time.sleep(0.1)
    shown = pg.inner_text("#next")
    pg.fill("#label", "Airline newsletter")
    pg.click("#make-ok")
    pg.wait_for_selector("#done:not([hidden])", timeout=10000)
    ok(pg.inner_text("#done-addr") == shown, "made the address it showed")
    pg.click("#done-ok")
    ok(
        pg.inner_text(".al-list li .al-label") == "Airline newsletter"
        and pg.inner_text("#count") == "1 of 100",
        "listed, and counted",
    )
    pg.click(".al-list li .switch")
    pg.wait_for_selector(".al-list li.off", timeout=10000)
    ok(mail(shown) == 400, "the switch turns it off")
    pg.click(".al-list li .switch")
    pg.wait_for_selector(".al-list li:not(.off)", timeout=10000)
    pg.click(".al-list li .al-more")
    pg.click(".al-pop .danger")
    pg.click(".al-yes")
    pg.wait_for_selector("text=No aliases yet", timeout=10000)
    ok(mail(shown) == 400, "deleted from the menu")
    ok(not errs, f"no errors on the aliases page {errs}")

    # one that turns itself off: a day is two seconds here (ALIAS_DAY)
    def listed(address):
        return next(
            (
                x
                for x in call("/api/aliases", cookie=sess)[1]["aliases"]
                if x["address"] == address
            ),
            {},
        )

    o = call("/api/aliases/next", {}, cookie=sess)[1]
    ok(
        call(
            "/api/aliases",
            {"n": o["n"], "day": o["day"], "label": "x", "ttl": 3},
            cookie=sess,
        )[0]
        == 400,
        "a time to turn off of its own choosing: refused",
    )
    s, v, _ = call(
        "/api/aliases",
        {"n": o["n"], "day": o["day"], "label": "Webinar", "ttl": 1},
        cookie=sess,
    )
    brief = v.get("alias", {}).get("address", "")
    bl = brief.split("@")[0]
    ok(
        s == 200 and v["alias"]["ends"] and v["alias"]["on"],
        f"made, to turn off after a day {v}",
    )
    ok(mail(brief) == 200, "until then, sent on")
    time.sleep(2.5)
    a = listed(brief)
    ok(
        not a["on"] and a["auto"] and a["off"] and not a["ends"],
        f"a day on, it has turned off on its own {a}",
    )
    ok(mail(brief) == 400, "and bounces")
    ok(
        call(f"/api/aliases/{bl}", {"ttl": 7}, cookie=sess)[0] == 409,
        "one already off is not given a day to turn off",
    )
    s, v, _ = call(f"/api/aliases/{bl}", {"on": True}, cookie=sess)
    ok(
        s == 200 and v["alias"]["on"] and not v["alias"]["ends"] and mail(brief) == 200,
        "switched on again, for good",
    )
    s, v, _ = call(f"/api/aliases/{bl}", {"ttl": 30}, cookie=sess)
    ok(s == 200 and v["alias"]["ends"], "a month to go, from the menu")
    s, v, _ = call(f"/api/aliases/{bl}", {"ttl": 0}, cookie=sess)
    ok(s == 200 and not v["alias"]["ends"], "and then never")
    s, v, _ = call(f"/api/aliases/{bl}", {"on": False, "ttl": 7}, cookie=sess)
    ok(
        s == 200 and not v["alias"]["on"] and mail(brief) == 400,
        "switched off with a day to turn off as well: off",
    )
    call(f"/api/aliases/{bl}", {"on": True}, cookie=sess)

    # the sites that mail it: the first is its own, a later one is new
    ok(
        listed(brief)["from"] == ["example.org"] and listed(brief)["fresh"] == [],
        f"the first site to mail it is its own {listed(brief)}",
    )
    mail(brief, sender='"Deals" <win@mail.dealsblast.biz>')
    mail(brief, sender="orders@shop.example.co.uk")
    mail(brief, sender="again@example.org")
    ok(
        listed(brief)["fresh"] == ["dealsblast.biz", "example.co.uk"],
        f"two new sites, by their own names {listed(brief)}",
    )
    s, v, _ = call(f"/api/aliases/{bl}", {"fine": True}, cookie=sess)
    ok(
        s == 200
        and v["alias"]["fresh"] == []
        and v["alias"]["from"] == ["example.org", "dealsblast.biz", "example.co.uk"],
        f"that's fine: they are its own now {v}",
    )

    # the owner's own aliases: the names of the headers that came, no more
    log = open("wrangler.log", errors="replace").read()
    names = re.findall(r"alias: header names (.*)", log)
    ok(
        names
        and "from" in names[-1].split()
        and all(re.fullmatch(r"[a-z0-9-]+( [a-z0-9-]+)*", n.strip()) for n in names),
        f"for the owner, the header names alone are logged {names[-1:]}",
    )

    # Passwords, as a SimpleLogin server: its own key, from any origin
    def sl(key, host="github.com", method="POST"):
        req = urllib.request.Request(
            f"{W}/api/alias/random/new?hostname={host}",
            data=b'{"note":"Generated by Bitwarden."}' if method == "POST" else None,
            method=method,
            headers={
                "authentication": key,
                "content-type": "application/json",
                "origin": "chrome-extension://abcdef",
            },
        )
        try:
            r = urllib.request.urlopen(req)
            return r.status, json.loads(r.read() or b"{}"), r.headers
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read() or b"{}"), e.headers

    ok(sl("cmty_" + "A" * 32)[0] == 401, "a key that is not ours: refused")
    ok(
        call("/api/aliases/passwords", {}, cookie=sess)[0] == 403,
        "connecting with the session alone: refused",
    )
    ok(call("/api/aliases", cookie=sess)[1]["passwords"] is None, "not connected yet")
    s, v, _ = call("/api/aliases/passwords", confirmed(), cookie=sess)
    key = v.get("key", "")
    ok(
        s == 200
        and re.fullmatch(r"cmty_[A-Za-z0-9_-]{32}", key)
        and v["url"] == "https://mail.commonty.test",
        f"connected: a key, shown once {s}",
    )
    s, _, hd = sl(key, method="OPTIONS")
    ok(
        s == 204 and hd.get("access-control-allow-origin") == "*",
        "the extension may ask, from its own origin",
    )
    s, v, hd = sl(key)
    made = v.get("alias", "")
    ok(
        s == 201
        and re.fullmatch(r"[a-z]{3,10}\.[a-z]{3,10}\.[0-9]{4}@commonty\.test", made),
        f"Passwords makes one {s} {v}",
    )
    ok(sl(key)[1].get("alias") == made, "the same site again soon: the same alias")
    other = sl(key, host="www.gitlab.com")[1].get("alias", "")
    ok(other and other != made, "another site: another alias")
    a = listed(made)
    ok(
        a.get("by") == "pw" and a.get("label") == "github.com",
        f"labelled with the site, made by Passwords {a}",
    )
    ok(listed(other).get("label") == "gitlab.com", "a site's www. left off")
    ok(mail(made) == 200, "and its mail is sent on")
    ok(
        call("/api/aliases", cookie=sess)[1]["passwords"]["used"],
        "the key's last use is shown",
    )
    s, v, _ = call("/api/aliases/passwords/delete", {}, cookie=sess)
    ok(
        s == 200 and v["passwords"] is None and sl(key, host="example.net")[0] == 401,
        "disconnected: the key makes no more",
    )

    # the page: the new choice in New alias, the row's small print, the settings
    pg.goto(f"{W}/aliases?name=tester&back={back}")
    pg.wait_for_selector("#new:not([disabled])", timeout=10000)
    ok(not pg.is_hidden("#opts"), "Alias settings are shown")
    ok("Made by Passwords" in pg.inner_text(".al-list"), "a row says Passwords made it")
    pg.click("#new")
    for _ in range(50):
        if "@" in pg.inner_text("#next"):
            break
        time.sleep(0.1)
    pg.fill("#label", "Conference")
    pg.select_option("#ttl", "7")
    pg.click("#make-ok")
    pg.wait_for_selector("#done:not([hidden])", timeout=10000)
    pg.click("#done-ok")
    ok(
        "turns off" in pg.inner_text(".al-list li:first-child .al-meta"),
        "made to turn off in a week",
    )
    pg.click(".al-list li:first-child .al-more")
    ok(pg.is_visible("text=Turn off on another day…"), "the menu can change the day")
    pg.click("text=Turn off on another day…")
    pg.click(".al-pop >> text=Never")
    for _ in range(50):
        if "turns off" not in pg.inner_text(".al-list li:first-child .al-meta"):
            break
        time.sleep(0.1)
    ok(
        "turns off" not in pg.inner_text(".al-list li:first-child .al-meta"),
        "and Never takes it away",
    )
    pg.click("#pw-connect")
    pg.wait_for_selector("#pw[open]", timeout=10000)
    ok(
        re.fullmatch(r"cmty_[A-Za-z0-9_-]{32}", pg.inner_text("#pw-key")),
        "Connect shows the key once",
    )
    pg.click("#pw-done")
    ok(
        pg.inner_text("#pw-key") == "" and not pg.is_hidden("#pw-on"),
        "and it is gone from the page, connected",
    )
    pg.click("#pw-disconnect")
    pg.wait_for_selector("#pw-off:not([hidden])", timeout=10000)
    ok(not errs, f"no errors on the aliases page, its settings either {errs}")
    # ten offers a day, each made at most once: the rest of today's made
    had = len(call("/api/aliases", cookie=sess)[1]["aliases"])
    more = 0
    for i in range(10):
        r = call("/api/aliases/next", {}, cookie=sess)
        if r[0] == 429:
            break
        call(
            "/api/aliases",
            {"n": r[1]["n"], "day": r[1]["day"], "label": f"n{i}"},
            cookie=sess,
        )
        more += 1
    # made before this: Shop and the page's, both deleted, and the ones kept
    ok(
        call("/api/aliases/next", {}, cookie=sess)[0] == 429 and 2 + had + more == 10,
        f"today's ten made: no eleventh offered ({had} kept, {more} more)",
    )
    ok(
        len(call("/api/aliases", cookie=sess)[1]["aliases"]) == had + more,
        "the ones still had, listed",
    )
    forged = sess[:-2] + ("AA" if not sess.endswith("AA") else "BB")
    ok(call("/api/me", cookie=forged)[0] == 401, "forged session: refused")
    # a box replaying an old entry after a newer one was seen
    fake("/entry", {"v": 4})
    ok(call("/api/challenge", {"name": "tester"})[0] == 200, "newer entry v4 taken")
    fake("/entry", {"v": 3})
    ok(
        call("/api/challenge", {"name": "tester"})[0] == 200,
        "old v3 replayed: still served from pinned v4",
    )
    # "sign out everywhere else" from Settings: the other sessions end, this one stays
    ok(call("/api/me", cookie=sess)[0] == 200, "the first session still works")
    # Passwords' key is held as a session is: today's ten are made, so a key
    # that still holds is told 429, and one that does not, 401
    key = call("/api/aliases/passwords", confirmed(), cookie=sess)[1].get("key", "")
    ok(sl(key, host="example.net")[0] == 429, "a new key holds")
    s, c, _ = call("/api/challenge", {"name": "tester"})
    sess2 = call(
        "/api/login",
        {"token": c["token"], "id": "cGsx", "assertion": sign(c["challenge"])},
    )[2].split(";")[0]
    ok(
        call("/api/end-others", {}, origin="http://127.0.0.1:8787", cookie=sess2)[0]
        == 403,
        "end-others from another origin: refused",
    )
    s, _, ck = call("/api/end-others", {}, cookie=sess2)
    ok(s == 200 and ck, "end-others from this browser")
    sess2 = ck.split(";")[0]
    ok(call("/api/me", cookie=sess)[0] == 401, "the other session is ended")
    ok(call("/api/me", cookie=sess2)[0] == 200, "this browser stays signed in")
    ok(
        sl(key, host="example.net")[0] == 401
        and call("/api/aliases", cookie=sess2)[1]["passwords"] is None,
        "signed out everywhere: Passwords' key is ended too, and shown so",
    )
    key = call("/api/aliases/passwords", confirmed(), cookie=sess2)[1].get("key", "")
    ok(sl(key, host="example.net")[0] == 429, "connected again from this browser")
    # the passkey it was opened with is removed from the entry: ended too
    fake("/entry", {"v": 5})
    ok(call("/api/me", cookie=sess2)[0] == 401, "its passkey removed: session ended")
    ok(
        sl(key, host="example.net")[0] == 401,
        "its passkey removed: Passwords' key ended",
    )
    # A box unlocking its disks at boot (unlock.js): signed as its TPM would
    from cryptography.hazmat.primitives import hashes as H, serialization as S
    from cryptography.hazmat.primitives.asymmetric import ec as EC

    boxkey = S.load_der_private_key(
        base64.b64decode(json.load(open(sys.argv[1]))["box_pkcs8"]), None
    )
    import time

    clock = [int(time.time()) - 100]

    def boxcall(path, kind, mac="", ip="203.0.113.5", extra=None, at=None, key=None):
        clock[0] += 1
        t = at if at is not None else clock[0]
        if kind == "share":
            msg = f"commonty unlock share v1\0testbox\0{t}\0{extra}"
        else:
            msg = f"commonty unlock{' checkin' if kind == 'checkin' else ''} v1\0testbox\0{t}\0{mac}"
        sig = b64u((key or boxkey).sign(msg.encode(), EC.ECDSA(H.SHA256())))
        body = {"box": "testbox", "at": t, "mac": mac, "sig": sig}
        if extra:
            body["share"] = extra
        req = urllib.request.Request(
            W + path,
            data=json.dumps(body).encode(),
            method="POST",
            headers={"content-type": "application/json", "cf-connecting-ip": ip},
        )
        try:
            r = urllib.request.urlopen(req)
            return r.status, json.load(r)
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read() or b"{}")

    HOME_MAC, AWAY_MAC = "aa:bb:cc:dd:ee:01", "aa:bb:cc:dd:ee:99"
    share = b64u(b"S" * 32)
    ok(
        boxcall("/api/unlock", "unlock", HOME_MAC)[0] == 404,
        "no half kept yet: nothing to give",
    )
    ok(
        boxcall("/api/unlock/share", "share", extra=share)[0] == 200,
        "the box keeps its half here",
    )
    ok(
        boxcall("/api/unlock/share", "share", extra=b64u(b"X" * 32))[0] == 409,
        "and cannot be made to swap it",
    )
    ok(
        boxcall("/api/unlock/checkin", "checkin", HOME_MAC)[0] == 200,
        "running at home, it says where home is",
    )
    s, v = boxcall("/api/unlock", "unlock", HOME_MAC)
    ok(s == 200 and v.get("share") == share, f"at home: the half {s}")
    s, v = boxcall("/api/unlock", "unlock", HOME_MAC, ip="198.51.100.7")
    ok(s == 200 and v.get("share") == share, f"new address, same router: the half {s}")
    # home is now that new address with the old router
    s, v = boxcall("/api/unlock", "unlock", AWAY_MAC, ip="198.51.100.7")
    ok(s == 200, f"same address, new router: the half {s}")
    t0 = clock[0]
    ok(
        boxcall("/api/unlock", "unlock", HOME_MAC, at=t0)[0] == 403,
        "a recorded request sent again: refused",
    )
    ok(
        boxcall(
            "/api/unlock",
            "unlock",
            HOME_MAC,
            key=EC.generate_private_key(EC.SECP256R1()),
        )[0]
        == 403,
        "signed by another key: refused",
    )
    s, v = boxcall("/api/unlock", "unlock", "aa:bb:cc:dd:ee:77", ip="192.0.2.50")
    ok(s == 202 and "share" not in v, f"somewhere new: waits, no half {s}")
    # a member lets it in, with their passkey just now
    s, c, _ = call("/api/challenge", {"name": "tester"})
    # the entry is at v5 now, where the passkey is cGsy
    s, _, ck = call(
        "/api/login",
        {"token": c["token"], "id": "cGsy", "assertion": sign(c["challenge"])},
    )
    m = ck.split(";")[0] if ck else ""
    ok(
        call(
            "/api/boxes/approve",
            {"box": "testbox"},
            origin="http://127.0.0.1:8787",
            cookie=m,
        )[0]
        == 403,
        "approving from another page: refused",
    )
    boxes = call("/api/boxes", cookie=m)[1]
    ok(
        isinstance(boxes, list) and boxes and boxes[0].get("waiting"),
        f"the member sees it waiting {boxes}",
    )
    ok(
        call("/api/boxes/approve", {"box": "testbox"}, cookie=m)[0] == 200,
        "the member lets it in",
    )
    s, v = boxcall("/api/unlock", "unlock", "aa:bb:cc:dd:ee:66", ip="192.0.2.51")
    ok(s == 202, f"approval is for where it waited, not anywhere {s}")
    s, v = boxcall("/api/unlock", "unlock", "aa:bb:cc:dd:ee:77", ip="192.0.2.50")
    ok(s == 200 and v.get("share") == share, f"approved: the half {s}")
    ok(
        call("/api/boxes/stolen", {"box": "testbox", "stolen": True}, cookie=m)[0]
        == 200,
        "marked stolen",
    )
    ok(
        boxcall("/api/unlock", "unlock", "aa:bb:cc:dd:ee:77", ip="192.0.2.50")[0]
        == 403,
        "stolen: refused even at its home",
    )
    ok(
        boxcall("/api/unlock/checkin", "checkin", HOME_MAC)[0] == 403,
        "stolen: cannot move home either",
    )
    ok(
        call("/api/boxes/stolen", {"box": "testbox", "stolen": False}, cookie=m)[0]
        == 200,
        "not stolen after all",
    )
    ok(
        boxcall("/api/unlock", "unlock", "aa:bb:cc:dd:ee:77", ip="192.0.2.50")[0]
        == 200,
        "and it unlocks again",
    )
    ok(call("/api/boxes", cookie="")[0] == 401, "nobody signed in sees no boxes")

    # A box's mail (send.js), signed as dd-sendmail signs it with the TPM
    import os, re

    def mailcall(to, msg, key=None, at=None, nonce=None, signed_to=None):
        t = at if at is not None else int(time.time())
        n = nonce or b64u(os.urandom(18))
        digest = hashlib.sha256(msg).hexdigest()
        m = f"commonty mail send v1\0testbox\0{t}\0{n}\0{signed_to or to}\0{digest}"
        sig = b64u((key or boxkey).sign(m.encode(), EC.ECDSA(H.SHA256())))
        req = urllib.request.Request(
            W + "/send",
            data=msg,
            method="POST",
            headers={
                "x-dd-box": "testbox",
                "x-dd-at": str(t),
                "x-dd-nonce": n,
                "x-dd-to": to,
                "x-dd-sig": sig,
            },
        )
        try:
            r = urllib.request.urlopen(req)
            return r.status, json.load(r), n, t
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read() or b"{}"), n, t

    def mails():
        # wrangler's send_email stand-in names each message's file in its log
        log = open("wrangler.log", errors="replace").read()
        return re.findall(r"(/\S+\.eml)", log)

    def newest(before):
        for _ in range(50):
            now = mails()
            if len(now) > len(before):
                return open(now[-1], "rb").read().decode()
            time.sleep(0.1)
        return ""

    msg = (
        b"From: Commonty Passwords <noreply@elsewhere.example>\n"
        b"To: someone@evil.example\n"
        b"Bcc: someone@evil.example\n"
        b"Reply-To: phish@evil.example\n"
        b"Subject: Your vault\n"
        b"Content-Type: text/plain; charset=utf-8\n"
        b"\n"
        b"Hello from the vault.\n"
    )
    before = mails()
    s, v, _, _ = mailcall("tester@commonty.test", msg)
    ok(s == 200 and v.get("sent") == 1, f"a box writes to a member {s} {v}")
    eml = newest(before)
    head = eml.split("\r\n\r\n")[0]
    ok(
        'From: "Commonty Passwords" <noreply@commonty.test>' in head
        and "To: <tester@commonty.test>" in head
        and "Subject: Your vault" in head
        and "Hello from the vault." in eml,
        f"from noreply@, to the member, subject and body kept {head!r}",
    )
    ok(
        "evil.example" not in eml and "elsewhere.example" not in eml,
        "the box's own To, Bcc, Reply-To and From are gone",
    )
    before = mails()
    s, v, nonce, t = mailcall("alerts", b"Subject: node1: backup FAILED\n\nlook\n")
    ok(s == 200, f"alerts, to the owner {s} {v}")
    ok(
        'From: "Commonty" <alerts@commonty.test>' in newest(before),
        "alerts come from alerts@",
    )
    ok(
        mailcall("alerts", b"Subject: x\n\nagain\n", nonce=nonce)[0] == 403,
        "the same nonce again: refused",
    )
    ok(
        mailcall("alerts", b"Subject: x\n\nold\n", at=int(time.time()) - 3600)[0]
        == 403,
        "an hour-old time: refused",
    )
    before = mails()
    ok(
        mailcall("someone@example.com", b"Subject: x\n\nhi\n")[0] == 403,
        "a stranger's address: refused",
    )
    ok(
        mailcall("other@commonty.test", b"Subject: x\n\nhi\n")[0] == 403,
        "a rule that is no member's: refused",
    )
    ok(
        mailcall("security@commonty.test", b"Subject: x\n\nhi\n")[0] == 403,
        "an alias: refused",
    )
    ok(
        mailcall(
            "tester@commonty.test",
            b"Subject: x\n\nhi\n",
            key=EC.generate_private_key(EC.SECP256R1()),
        )[0]
        == 403,
        "signed by another key: refused",
    )
    ok(
        mailcall(
            "someone@example.com",
            b"Subject: x\n\nhi\n",
            signed_to="tester@commonty.test",
        )[0]
        == 403,
        "recipients changed after signing: refused",
    )
    ok(
        mailcall("tester@commonty.test", b"x" * (300 * 1024))[0] == 413,
        "too big: refused",
    )
    ok(len(mails()) == len(before), "nothing refused was sent")
    # SEND_PER_RCPT 4 and SEND_PER_BOX 6 here (mail-worker.nix): tester has
    # one, alerts one
    codes = [
        mailcall("tester@commonty.test", b"Subject: n\n\nn\n")[0] for _ in range(4)
    ]
    ok(
        codes == [200, 200, 200, 429],
        f"a fifth to one member in the hour: refused {codes}",
    )
    codes = [mailcall("alerts", b"Subject: n\n\nn\n")[0] for _ in range(2)]
    ok(codes == [200, 429], f"a seventh from the box in the hour: refused {codes}")

    # A member's locked inbox (inbox.js). The key is made here as the app
    # makes it; what the Worker locks is opened by HPKE written apart from
    # hpke.js (cryptography's X25519 and AES-GCM), so the two must agree.
    import hmac
    from cryptography.hazmat.primitives.asymmetric.x25519 import (
        X25519PrivateKey,
        X25519PublicKey,
    )
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM

    def hk_extract(salt, ikm):
        return hmac.new(salt or bytes(32), ikm, hashlib.sha256).digest()

    def hk_expand(prk, info, n):
        out, t, i = b"", b"", 1
        while len(out) < n:
            t = hmac.new(prk, t + info + bytes([i]), hashlib.sha256).digest()
            out += t
            i += 1
        return out[:n]

    KEM = b"KEM" + (0x20).to_bytes(2, "big")
    SUITE = (
        b"HPKE"
        + (0x20).to_bytes(2, "big")
        + (1).to_bytes(2, "big")
        + (2).to_bytes(2, "big")
    )

    def lx(suite, salt, label, ikm):
        return hk_extract(salt, b"HPKE-v1" + suite + label + ikm)

    def le(suite, prk, label, info, n):
        return hk_expand(
            prk, n.to_bytes(2, "big") + b"HPKE-v1" + suite + label + info, n
        )

    def hpke_open(sk, info, blob):
        assert blob[:4] == b"CML1", "not a locked message"
        enc, ct = blob[4:36], blob[36:]
        pk = sk.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)
        dh = sk.exchange(X25519PublicKey.from_public_bytes(enc))
        ss = le(KEM, lx(KEM, b"", b"eae_prk", dh), b"shared_secret", enc + pk, 32)
        ctx = (
            b"\0"
            + lx(SUITE, b"", b"psk_id_hash", b"")
            + lx(SUITE, b"", b"info_hash", info.encode())
        )
        secret = lx(SUITE, ss, b"secret", b"")
        key = le(SUITE, secret, b"key", ctx, 32)
        nonce = le(SUITE, secret, b"base_nonce", ctx, 12)
        return AESGCM(key).decrypt(nonce, ct, b"")

    # signed in just now, with the passkey the entry has by this point (cGsy)
    s, c, _ = call("/api/challenge", {"name": "tester"})
    s, _, ck = call(
        "/api/login",
        {"token": c["token"], "id": "cGsy", "assertion": sign(c["challenge"])},
    )
    isess = ck.split(";")[0] if ck else ""
    ok(s == 200 and isess, "signed in for the inbox")
    mailsk = X25519PrivateKey.generate()
    mailpk = b64u(mailsk.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw))
    # the made-up member's own device, as their entry names it (fixture.rs)
    devsk = Ed25519PrivateKey.from_private_bytes(bytes([7] * 32))
    devpub = base64.b64encode(
        devsk.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)
    ).decode()
    dev = {"fp": "test", "key": devpub}
    keysig = lambda pk, sk=devsk: base64.b64encode(
        sk.sign(f"commonty mail key v1\0tester\0{pk}".encode())
    ).decode()

    s, v, _ = call("/api/inbox", cookie=isess)
    ok(s == 200 and v["on"] is False and v["fwd"] is True, f"no inbox yet {v}")
    ok(
        call("/api/inbox", {"pk": mailpk, "sig": keysig(mailpk), "device": dev})[0]
        == 401,
        "no session: no inbox",
    )
    ok(
        call("/api/inbox", {"pk": "short", "device": dev}, cookie=isess)[0] == 400,
        "a key that is not one: refused",
    )
    # a link someone else made: their device, or the member's device with their key
    other_dev = Ed25519PrivateKey.generate()
    odpub = base64.b64encode(
        other_dev.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)
    ).decode()
    ok(
        call(
            "/api/inbox",
            {
                "pk": mailpk,
                "sig": keysig(mailpk, other_dev),
                "device": {"fp": "evil1", "key": odpub},
            },
            cookie=isess,
        )[0]
        == 403,
        "a device not in their entry: refused",
    )
    evilpk = b64u(
        X25519PrivateKey.generate()
        .public_key()
        .public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)
    )
    ok(
        call(
            "/api/inbox",
            {"pk": evilpk, "sig": keysig(mailpk), "device": dev},
            cookie=isess,
        )[0]
        == 403,
        "a key their device did not sign: refused",
    )
    s, v, _ = call(
        "/api/inbox", {"pk": mailpk, "sig": keysig(mailpk), "device": dev}, cookie=isess
    )
    ok(
        s == 200 and v["on"] and v["fwd"] and v["devices"] == [dev["fp"]],
        f"inbox on {s} {v}",
    )
    other = b64u(
        X25519PrivateKey.generate()
        .public_key()
        .public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)
    )
    ok(
        call(
            "/api/inbox",
            {"pk": other, "sig": keysig(other), "device": dev},
            cookie=isess,
        )[0]
        == 409,
        "another key while one is on: refused",
    )
    st = fake("/state")["rules"].values()
    mine = [r for r in st if r["matchers"][0]["value"] == "tester@commonty.test"]
    ok(
        any(
            r["name"] == "Commonty inbox tester"
            and r["actions"] == [{"type": "worker", "value": ["mail-e2e"]}]
            and r["enabled"]
            for r in mine
        )
        and any(
            r["name"] == "Commonty member tester"
            and r["enabled"] is False
            and r["actions"][0]["value"] == ["second@example.com"]
            for r in mine
        ),
        f"their address now comes here, their rule kept off as where it forwards {mine}",
    )

    hclock = [int(time.time() * 1000) - 60000]

    def holdcall(path, method="GET", key=None, at=None):
        hclock[0] += 1
        t = at if at is not None else hclock[0]
        m = f"commonty mail hold v1\0testbox\0{t}\0{method}\0{path}\0{hashlib.sha256(b'').hexdigest()}"
        sig = b64u((key or boxkey).sign(m.encode(), EC.ECDSA(H.SHA256())))
        req = urllib.request.Request(
            W + path,
            data=b"" if method == "POST" else None,
            method=method,
            headers={"x-dd-box": "testbox", "x-dd-at": str(t), "x-dd-sig": sig},
        )
        try:
            r = urllib.request.urlopen(req)
            body = r.read()
            return r.status, body
        except urllib.error.HTTPError as e:
            return e.code, e.read()

    def held():
        s, body = holdcall("/api/hold/list?after=")
        return json.loads(body)["items"] if s == 200 else None

    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    signer = Ed25519PublicKey.from_public_bytes(
        base64.b64decode(open("mailsign.pub").read())
    )

    def unpack(blob):
        assert blob[:4] == b"CMA1", "not a locked object"
        hl = int.from_bytes(blob[4:8], "big")
        h = json.loads(blob[8 : 8 + hl])
        sl = int.from_bytes(blob[8 + hl : 12 + hl], "big")
        return h, blob[12 + hl : 12 + hl + sl], blob[12 + hl + sl :]

    def take(item):
        blob = b"".join(
            holdcall(f"/api/hold/get?id={item['id']}&n={n}")[1]
            for n in range(item["parts"])
        )
        h, S, R = unpack(blob)
        # signed by the Worker, over what it sealed, for this id
        sha = lambda b: (
            base64.urlsafe_b64encode(hashlib.sha256(b).digest()).rstrip(b"=").decode()
        )
        assert (
            h["kind"] == "in"
            and h["id"] == item["id"]
            and h["hs"] == sha(S)
            and h["hr"] == sha(R)
            and h["by"] == "worker"
        ), h
        signer.verify(
            base64.b64decode(h["sig"]),
            f"commonty mail object v1\0in\0{h['id']}\0{h['hs']}\0{h['hr']}".encode(),
        )
        summ = json.loads(
            hpke_open(mailsk, f"commonty mail v1 summary {item['id']}", S)
        )
        raw = hpke_open(mailsk, f"commonty mail v1 raw {item['id']}", R)
        return summ, raw

    ok(held() == [], "nothing held yet")
    n0 = forwarded()
    ok(
        mail(
            "tester@commonty.test",
            head="Subject-Extra: x\r\n",
            body="the locked body\r\n",
        )
        == 200,
        "mail to their address taken",
    )
    items = held()
    ok(
        items and len(items) == 1 and items[0]["member"] == "tester",
        f"held for them {items}",
    )
    summ, raw = take(items[0])
    ok(
        summ["subject"] == "hi"
        and summ["from"]["address"] == "shop@example.org"
        and summ["alias"] is None
        and summ["id"] == items[0]["id"],
        f"its summary opens with their key {summ}",
    )
    ok(
        b"the locked body" in raw and raw.startswith(b"From: shop@example.org"),
        "the whole message opens with their key",
    )
    ok(forwarded() == n0 + 1, "and it went on to their address as well")
    hpke_err = None
    try:
        hpke_open(
            mailsk,
            f"commonty mail v1 raw {items[0]['id']}x",
            unpack(holdcall(f"/api/hold/get?id={items[0]['id']}&n=0")[1])[2],
        )
    except Exception as e:
        hpke_err = e
    ok(hpke_err is not None, "locked to its own id: it does not open as another")
    ok(
        holdcall(f"/api/hold/ack?id={items[0]['id']}", "POST")[0] == 200,
        "the box says it has it",
    )
    ok(held() == [], "and it is held no more")

    # unsigned, signed by another key, an old time again: refused
    req = urllib.request.Request(W + "/api/hold/list?after=")
    try:
        urllib.request.urlopen(req)
        ok(False, "unsigned hold list refused")
    except urllib.error.HTTPError as e:
        ok(e.code == 403, "unsigned: refused")
    ok(
        holdcall("/api/hold/list?after=", key=EC.generate_private_key(EC.SECP256R1()))[
            0
        ]
        == 403,
        "another key: refused",
    )
    ok(
        holdcall("/api/hold/list?after=", at=hclock[0] - 5)[0] == 403,
        "an older time than its last: refused",
    )
    ok(
        holdcall("/api/hold/list?after=", at=int(time.time() * 1000) - 3600_000)[0]
        == 403,
        "an hour old: refused",
    )

    # their alias's mail is held too, and says which alias
    # one of theirs that is on (the day's ten new ones were spent above)
    alias = next(
        x["address"]
        for x in call("/api/aliases", cookie=isess)[1]["aliases"]
        if x["on"]
    )
    ok(mail(alias) == 200, "mail to an alias taken")
    items = held()
    summ, _ = take(items[0])
    ok(
        summ["alias"] == alias.split("@")[0],
        f"held, with the alias it came to {summ.get('alias')}",
    )
    holdcall(f"/api/hold/ack?id={items[0]['id']}", "POST")

    # the sender's word on itself is not Cloudflare's: only the results under
    # Cloudflare's own name, on top, count
    ok(
        mail(
            "tester@commonty.test",
            head="Authentication-Results: mx.cloudflare.net; spf=fail; dmarc=fail\r\nARC-Authentication-Results: i=1; dmarc=pass\r\n",
        )
        == 200,
        "a forged sender",
    )
    summ, _ = take(held()[0])
    ok(summ["auth"]["dmarc"] == "fail", f"its DMARC failure kept {summ['auth']}")
    holdcall(f"/api/hold/ack?id={held()[0]['id']}", "POST")
    ok(
        mail(
            "tester@commonty.test",
            head="ARC-Authentication-Results: i=1; mx.cloudflare.net; dmarc=pass\r\nAuthentication-Results: evil.example; dmarc=pass\r\n",
        )
        == 200,
        "a sender's own results",
    )
    summ, _ = take(held()[0])
    ok(summ["auth"]["dmarc"] == "", f"not taken at its word {summ['auth']}")
    holdcall(f"/api/hold/ack?id={held()[0]['id']}", "POST")

    # too big to lock (LOCK_MAX 4000 here): a locked note in the inbox, and
    # the message whole to their address, even with forwarding off later
    n0 = forwarded()
    ok(mail("tester@commonty.test", body="x" * 6000 + "\r\n") == 200, "a big one taken")
    items = held()
    summ, note = take(items[0])
    ok(
        summ.get("tooBig")
        and summ["subject"] == "hi"
        and b"more than can be locked" in note
        and b"xxxx" not in note,
        f"held as a note of what came {summ.get('snippet')}",
    )
    ok(forwarded() == n0 + 1, "and sent on whole")
    holdcall(f"/api/hold/ack?id={items[0]['id']}", "POST")

    # forwarding off: held, not sent on
    ok(
        call("/api/inbox/forward", {"fwd": False}, cookie=isess)[1].get("fwd") is False,
        "forwarding turned off",
    )
    n0 = forwarded()
    ok(
        mail("tester@commonty.test") == 200 and len(held()) == 1 and forwarded() == n0,
        "held and not forwarded",
    )
    holdcall(f"/api/hold/ack?id={held()[0]['id']}", "POST")
    ok(mail(alias) == 200 and len(held()) == 1 and forwarded() == n0, "an alias's too")
    holdcall(f"/api/hold/ack?id={held()[0]['id']}", "POST")
    ok(
        mail("tester@commonty.test", body="y" * 6000 + "\r\n") == 200
        and forwarded() == n0 + 1,
        "too big, with forwarding off: sent on all the same",
    )
    holdcall(f"/api/hold/ack?id={held()[0]['id']}", "POST")

    # the device: another member's key to lock to, and DKIM for mail it sends
    def devcall(path, body=None, key=None, fp=None, name="tester", at=None):
        t = at if at is not None else int(time.time())
        data = json.dumps(body).encode() if body is not None else b""
        method = "POST" if body is not None else "GET"
        m = f"commonty mail device v1\0{name}\0{fp or dev['fp']}\0{t}\0{method}\0{path}\0{hashlib.sha256(data).hexdigest()}"
        sig = base64.b64encode((key or devsk).sign(m.encode())).decode()
        req = urllib.request.Request(
            W + path,
            data=data if body is not None else None,
            method=method,
            headers={
                "content-type": "application/json",
                "x-dd-name": name,
                "x-dd-device": fp or dev["fp"],
                "x-dd-at": str(t),
                "x-dd-sig": sig,
            },
        )
        try:
            r = urllib.request.urlopen(req)
            return r.status, json.load(r)
        except urllib.error.HTTPError as e:
            return e.code, json.loads(e.read() or b"{}")

    s, v = devcall("/api/mailkey/tester")
    ok(
        s == 200 and v["pk"] == mailpk and v["devices"][0]["fp"] == dev["fp"],
        f"a member's key, to a device of theirs {s}",
    )
    ok(
        [w["pub"] for w in v.get("worker", [])] == [open("mailsign.pub").read()],
        "and the key the Worker signs with",
    )
    ok(
        devcall("/api/mailkey/tester", key=Ed25519PrivateKey.generate())[0] == 401,
        "signed by a device not confirmed here: refused",
    )
    ok(
        devcall("/api/mailkey/tester", at=int(time.time()) - 3600)[0] == 401,
        "an hour-old signature: refused",
    )
    ok(devcall("/api/mailkey/nobody")[0] == 404, "no inbox, no key")
    s, v = devcall("/api/addresses")
    ok(
        s == 200
        and v["address"] == "tester@commonty.test"
        and alias in [x["address"] for x in v["aliases"]],
        f"what they may send from {v}",
    )
    ok(
        devcall("/api/addresses", key=Ed25519PrivateKey.generate())[0] == 401,
        "not a device of theirs: refused",
    )

    import dkim as dkimpy

    pub = open("dkim.pub").read()

    def dns(name, timeout=5):
        return (
            f"v=DKIM1; k=rsa; p={pub}".encode()
            if name.decode().startswith("cm1._domainkey.commonty.test")
            else None
        )

    def signed(
        frm, to="friend@example.net", subj="Lunch?", body=b"See you at noon.\r\n"
    ):
        bh = base64.b64encode(
            hashlib.sha256(body.rstrip(b"\r\n") + b"\r\n").digest()
        ).decode()
        hdrs = [
            ["From", frm],
            ["To", to],
            ["Date", "Sun, 18 Oct 2026 10:00:00 +0000"],
            ["Message-ID", f"<{time.time_ns()}@commonty.test>"],
            ["MIME-Version", "1.0"],
            ["Content-Type", "text/plain; charset=utf-8"],
        ]
        s, v = devcall("/api/dkim", {"headers": hdrs, "bh": bh})
        if s != 200:
            return s, None
        msg = (
            v["header"]
            + "\r\n"
            + "".join(f"{n}: {x}\r\n" for n, x in hdrs)
            + f"Subject: {subj}\r\n\r\n"
        ).encode() + body
        return s, msg

    s, msg = signed("Tester <tester@commonty.test>")
    ok(
        s == 200 and dkimpy.verify(msg, dnsfunc=dns),
        f"from their address: a DKIM signature that verifies {s}",
    )
    ok(
        b" h=from:to:date:message-id:mime-version:content-type:from:to:cc:reply-to:date:message-id:in-reply-to:references:sender;"
        in msg
        and b"subject" not in msg.split(b"\r\n")[0].lower(),
        "the subject is neither signed nor sent here; the rest signed once more than there are",
    )
    fake2 = b"From: victim@commonty.test\r\n" + msg
    ok(
        not dkimpy.verify(fake2, dnsfunc=dns),
        "a second From added above: the signature breaks",
    )
    for bad in [
        '"someone@commonty.test" <tester@commonty.test>',
        "=?utf-8?q?someone=40commonty.test?= <tester@commonty.test>",
        "tester@commonty.test, someone@commonty.test",
    ]:
        ok(signed(bad)[0] == 400, f"a From that shows another address: refused {bad}")
    s, msg = signed(f"Tester <{alias}>")
    ok(
        s == 200 and dkimpy.verify(msg, dnsfunc=dns),
        f"from an alias of theirs: signed {s}",
    )
    ok(
        signed("someone@commonty.test")[0] == 403,
        "from another member's address: refused",
    )
    ok(signed("tester@example.com")[0] == 403, "from another domain: refused")
    ok(
        signed("brave.otter.1234@commonty.test")[0] == 403,
        "from an alias not theirs: refused",
    )
    # the same signed request sent again: refused
    body = {
        "headers": [["From", "tester@commonty.test"], ["To", "x@example.net"]],
        "bh": "A" * 43 + "=",
    }
    t = int(time.time())
    data = json.dumps(body).encode()
    m = f"commonty mail device v1\0tester\0{dev['fp']}\0{t}\0POST\0/api/dkim\0{hashlib.sha256(data).hexdigest()}"
    hd = {
        "content-type": "application/json",
        "x-dd-name": "tester",
        "x-dd-device": dev["fp"],
        "x-dd-at": str(t),
        "x-dd-sig": base64.b64encode(devsk.sign(m.encode())).decode(),
    }
    first = urllib.request.urlopen(
        urllib.request.Request(W + "/api/dkim", data=data, method="POST", headers=hd)
    ).status
    try:
        urllib.request.urlopen(
            urllib.request.Request(
                W + "/api/dkim", data=data, method="POST", headers=hd
            )
        )
        again = 200
    except urllib.error.HTTPError as e:
        again = e.code
    ok(
        (first, again) == (200, 401),
        f"a signed request sent twice: the second refused {first} {again}",
    )
    ok(
        signed("tester@commonty.test")[0] == 429,
        "a fourth today (SEND_PER_DAY 3): refused",
    )
    ok(
        devcall(
            "/api/dkim",
            {
                "headers": [["From", "tester@commonty.test"], ["X-Evil", "1"]],
                "bh": "A" * 43 + "=",
            },
        )[0]
        == 400,
        "a header not on the list: refused",
    )

    # off: their rule back on, the inbox's gone, nothing held for them
    mail("tester@commonty.test")
    s, v, _ = call("/api/inbox/off", {}, cookie=isess)
    ok(s == 200 and v["on"] is False, f"inbox off {s} {v}")
    mine = [
        r
        for r in fake("/state")["rules"].values()
        if r["matchers"][0]["value"] == "tester@commonty.test"
    ]
    ok(
        len(mine) == 1
        and mine[0]["enabled"] is not False
        and mine[0]["name"] == "Commonty member tester",
        f"their own rule forwards again {mine}",
    )
    ok(held() == [], "what was waiting for a box is gone")
    ok(
        devcall("/api/mailkey/tester")[0] == 401,
        "their device is no longer one to believe",
    )

    # the page the app opens to turn it on (inbox.html): the code both show,
    # the passkey on this Worker's own page
    import urllib.parse

    frag = urllib.parse.urlencode(
        {
            "name": "tester",
            "pk": mailpk,
            "fp": dev["fp"],
            "key": dev["key"],
            "sig": keysig(mailpk),
            "back": "https://files.commonty.test/_dd/mail",
        }
    )
    # the entry is at v5 by now, where the passkey is cGsy
    ictx = b.new_context(viewport={"width": 1000, "height": 760})
    ictx.expose_function("__sign", lambda c: {**sign(c), "id": "cGsy"})
    ictx.add_init_script(STUB)
    ip = ictx.new_page()
    ierrs = []
    ip.on("pageerror", lambda e: ierrs.append(str(e)))
    ip.goto(f"{W}/inbox#{frag}")
    want = hashlib.sha256(f"tester\0{dev['fp']}\0{mailpk}".encode()).hexdigest()[:12]
    want = "-".join(want[i : i + 4] for i in range(0, 12, 4))
    for _ in range(50):
        if ip.inner_text("#code"):
            break
        time.sleep(0.1)
    ok(
        ip.inner_text("#code") == want,
        f"the code is the device's and the key's {ip.inner_text('#code')} {want}",
    )
    ok("tester@commonty.test" in ip.inner_text("#addr"), "says whose address")
    ip.uncheck("#fwd")
    ip.click("#pass")
    try:
        ip.wait_for_selector("#s2:not([hidden])", timeout=10000)
    except Exception:
        ok(False, f"turned on from the page: {ip.inner_text('#s1-msg')}")
    s, v, _ = call("/api/inbox", cookie=isess)
    ok(
        v["on"] and v["fwd"] is False and v["pk"] == mailpk,
        f"turned on from the page, no copy forwarded {v}",
    )
    ok(not ierrs, f"no errors on the inbox page {ierrs}")
    ip.goto(f"{W}/inbox")
    ok(
        not ip.is_hidden("#s0") and ip.is_hidden("#s1"),
        "opened by hand, it says to open it from Mail",
    )
    call("/api/inbox/off", {}, cookie=isess)
    b.close()
if failed:
    sys.exit(f"{len(failed)} failed")
print("all passed")
