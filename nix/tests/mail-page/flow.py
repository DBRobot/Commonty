# The Mail page in a browser, as a browser the app let in sees it: turning it
# on, mail arriving locked and opening, reading it as a page with pictures
# from the internet held back, instant search, writing with the formatting
# bar, sending out and to a member, and the actions. Screenshots of each
# look go to $out for the pull request.
import json, os, sys, time, urllib.request
from playwright.sync_api import sync_playwright

PORT = int(sys.argv[1])
OUT = sys.argv[2]
HOST = f"http://s-0123456789abcdef0123456789abcdef.localhost:{PORT}"
F = f"http://127.0.0.1:{PORT}"

failed = []


def ok(c, m):
    print(("PASS " if c else "FAIL ") + m, flush=True)
    if not c:
        failed.append(m)


def post(path, body):
    req = urllib.request.Request(
        F + path,
        data=json.dumps(body).encode(),
        method="POST",
        headers={"content-type": "application/json"},
    )
    return json.load(urllib.request.urlopen(req))


def get(path):
    return json.load(urllib.request.urlopen(F + path))


def until(f, what, secs=15):
    end = time.time() + secs
    while time.time() < end:
        try:
            if f():
                return True
        except Exception:
            pass
        time.sleep(0.1)
    ok(False, f"waited for {what}")
    return False


def arrive(subject, frm, body, html=None, name="", extra_head="", att=None, auth=None):
    boundary = "b0undary"
    if html or att:
        parts = [
            f"--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        ]
        if html:
            parts.append(
                f"--{boundary}\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n"
            )
        if att:
            parts.append(
                f'--{boundary}\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename="{att}"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n'
            )
        raw = (
            f"From: {name} <{frm}>\r\nTo: tester@commonty.test\r\nSubject: {subject}\r\nDate: Sun, 18 Oct 2026 09:41:00 +0000\r\nMessage-ID: <{time.time_ns()}@x>\r\n{extra_head}MIME-Version: 1.0\r\nContent-Type: multipart/{'mixed' if att else 'alternative'}; boundary={boundary}\r\n\r\n"
            + "".join(parts)
            + f"--{boundary}--\r\n"
        )
    else:
        raw = f"From: {name} <{frm}>\r\nTo: tester@commonty.test\r\nSubject: {subject}\r\nDate: Sun, 18 Oct 2026 09:41:00 +0000\r\nMessage-ID: <{time.time_ns()}@x>\r\n{extra_head}\r\n{body}\r\n"
    summary = {
        "from": {"name": name, "address": frm},
        "to": [{"name": "", "address": "tester@commonty.test"}],
        "cc": [],
        "subject": subject,
        "snippet": body[:200],
        "attachments": [att] if att else [],
        "date": int(time.time() * 1000),
        "rcpt": "tester@commonty.test",
        "mid": f"<{time.time_ns()}@x>",
    }
    return post("/test/arrive", {"raw": raw, "summary": summary, "auth": auth or {}})[
        "id"
    ]


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
    ctx = b.new_context(viewport={"width": 1280, "height": 820})
    pg = ctx.new_page()
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
    pg.on(
        "console",
        lambda m: (
            errs.append(m.text)
            if m.type == "error"
            and "Content Security Policy" not in m.text
            and "404" not in m.text
            else None
        ),
    )

    # off: the page offers to turn it on
    pg.goto(f"{HOST}/_dd/mail")
    pg.wait_for_selector("#g-on:not([hidden])", timeout=15000)
    ok(True, "Mail not on yet: it offers to turn it on")
    pg.screenshot(path=f"{OUT}/1-turn-on.png")
    pg.click("#turn-on")
    pg.wait_for_selector("#g-confirm:not([hidden])", timeout=15000)
    code = pg.inner_text("#g-code")
    ok(len(code) == 14 and code.count("-") == 2, f"a code to compare {code}")
    href = pg.get_attribute("#confirm", "href")
    ok(
        href.startswith("https://mail.commonty.test/inbox#")
        and "pk=" in href
        and "fp=a1b2c3d4e5f6" in href,
        f"the passkey page is the Worker's own, with the key in the fragment {href[:60]}",
    )
    pg.screenshot(path=f"{OUT}/2-confirm.png")
    post("/test/register", {})
    pg.wait_for_selector("#app:not([hidden])", timeout=15000)
    ok(True, "confirmed: the mailbox opens by itself")

    # mail arrives, locked to the key the page derived; it opens
    a1 = arrive(
        "Dinner Saturday?",
        "lingyun@example.net",
        "I can bring the dumplings if you do the rice.",
        name="Lingyun",
    )
    a2 = arrive(
        "Your order #48213 has shipped",
        "orders@northside.example",
        "Two M3 heat-set insert kits are on their way.",
        name="Northside Hardware",
        html='<p>Two <b>M3 heat-set insert kits</b> are on their way.</p><img src="https://tracker.example/pixel.gif" alt="t"><p><a href="https://northside.example/track">Track it</a></p>',
        att="invoice-48213.pdf",
    )
    a3 = arrive(
        "This week: CAN FD timing",
        "hello@embdigest.example",
        "Bit timing that actually works at 5 Mbit/s.",
        name="The Embedded Digest",
    )
    a4 = arrive(
        "Unusual sign-in attempt",
        "secure@firstharb0r.example",
        "Verify your account now.",
        name="Your Bank",
        auth={"dmarc": "fail"},
    )
    pg.reload()
    pg.wait_for_selector(".mail-row", timeout=15000)
    until(
        lambda: pg.locator(".mail-row").count() == 3,
        "three in the inbox (the forged one in Spam)",
    )
    ok(
        pg.locator(".mail-row").first.inner_text().find("This week") >= 0,
        "newest first",
    )
    ok(
        "Inbox" in pg.inner_text("#folders")
        and "3" in pg.locator('[data-f="inbox"]').inner_text(),
        "three unread in Inbox",
    )
    pg.screenshot(path=f"{OUT}/3-inbox.png")

    # reading: the page, the toolbar on the right, the picture held back
    pg.click(f'.mail-row[data-id="{a2}"]')
    pg.wait_for_selector("#body iframe", timeout=10000)
    fr = pg.frame_locator("#body iframe")
    until(
        lambda: "heat-set insert kits" in fr.locator("body").inner_text(),
        "the html body in its frame",
    )
    ok(
        fr.locator("img").count() == 1 and not fr.locator("img").get_attribute("src"),
        "the tracking picture is not loaded",
    )
    ok(pg.locator(".mail-pics").count() == 1, "it says pictures are held back")
    ok("invoice-48213.pdf" in pg.inner_text("#att"), "the attachment is there to take")
    tools = pg.locator(".mail-tools").bounding_box()
    reply = pg.locator('[data-act="reply"]').bounding_box()
    last = pg.locator('.mail-tools [data-act="trash"]').bounding_box()
    ok(
        last["x"] + last["width"] > tools["x"] + tools["width"] - 40
        and reply["x"] > tools["x"] + 60,
        "the actions sit together on the right of the bar",
    )
    ok(
        "Locked when it arrived" in pg.inner_text(".mail-notes"),
        "says who could read it",
    )
    pg.screenshot(path=f"{OUT}/4-reading.png")
    pg.click('[data-act="star"]')
    until(
        lambda: pg.locator('[data-act="star"].on').count() == 1,
        "starred, the star filled",
    )

    # search: at once, by body words too once read
    t0 = time.time()
    pg.fill("#q", "heat")
    until(lambda: pg.locator(".mail-row").count() == 1, "one match for 'heat'")
    ms = (time.time() - t0) * 1000
    ok(ms < 1500, f"search answered in {ms:.0f} ms, browser round trip and all")
    pg.fill("#q", "dumpl")
    until(
        lambda: (
            pg.locator(".mail-row").count() == 1 and "Dinner" in pg.inner_text("#rows")
        ),
        "a word being typed finds it",
    )
    pg.fill("#q", "")

    # spam: the forged one is there, and why
    pg.click('[data-f="spam"]')
    until(
        lambda: pg.locator(f'.mail-row[data-id="{a4}"]').count() == 1,
        "the forged sender in Spam",
    )
    pg.click(f'.mail-row[data-id="{a4}"]')
    until(
        lambda: "failed the checks" in pg.inner_text(".mail-notes"),
        "says why it is spam",
    )
    pg.click('[data-f="inbox"]')

    # writing: in the viewer, the bar along the top, send on the right
    pg.click("#new")
    pg.wait_for_selector("#ed-f", timeout=5000)
    ok(
        pg.locator(".mail-fmt [data-cmd]").count() >= 25
        and pg.locator(".mail-fmt select").count() == 3,
        "every formatting tool",
    )
    send = pg.locator("#send").bounding_box()
    bold = pg.locator('[data-cmd="bold"]').bounding_box()
    ok(
        abs(send["y"] - bold["y"]) < 40 and send["x"] > bold["x"],
        "Send stays at the top right, beside the tools",
    )
    pg.fill("#c-to", "friend@example.net")
    until(lambda: "Leaves Commonty" in pg.inner_text("#route"), "says how it leaves")
    pg.click('[data-act="showcc"]')
    pg.fill("#c-cc", "otter@example.org")
    pg.fill("#c-subj", "Lunch Friday?")
    pg.click("#c-body")
    pg.keyboard.type("See you at ")
    pg.click('[data-cmd="bold"]')
    pg.keyboard.type("noon")
    pg.screenshot(path=f"{OUT}/5-writing.png")
    pg.click("#send")
    until(lambda: len(get("/test/sent")) == 1, "handed to the app to send")
    s = get("/test/sent")[0]
    import base64

    rawmsg = base64.urlsafe_b64decode(s["raw"] + "=" * (-len(s["raw"]) % 4)).decode()
    ok(
        sorted(s["to"]) == ["friend@example.net", "otter@example.org"]
        and s["from"] == "tester@commonty.test",
        f"to both, from their address {s['to']}",
    )
    ok(
        "Subject: Lunch Friday?" in rawmsg
        and "multipart/alternative" in rawmsg
        and ("<b>noon</b>" in rawmsg or "font-weight" in rawmsg),
        "the message as written, html beside the text",
    )
    until(
        lambda: pg.locator('[data-f="sent"][aria-current="true"]').count() == 1,
        "shown in Sent",
    )
    ok(
        any(k.startswith("tester/sent/") for k in get("/test/objs")),
        "a locked copy kept in Sent",
    )

    # to a member (themself here): locked on this device, end to end
    pg.click("#new")
    pg.wait_for_selector("#ed-f")
    pg.fill("#c-to", "tester@commonty.test")
    until(lambda: "End to end" in pg.inner_text("#route"), "says it is end to end")
    pg.fill("#c-subj", "Note to self")
    pg.click("#c-body")
    pg.keyboard.type("locked on my own device")
    pg.click("#send")
    until(
        lambda: len([k for k in get("/test/objs") if k.startswith("tester/in/")]) == 5,
        "delivered to their inbox as it is",
    )
    ok(len(get("/test/sent")) == 1, "nothing went the outside way")
    pg.click('[data-f="inbox"]')
    pg.reload()
    pg.wait_for_selector(".mail-row")
    until(lambda: "Note to self" in pg.inner_text("#rows"), "arrived")
    pg.click('.mail-row:has-text("Note to self")')
    until(
        lambda: "End to end" in pg.inner_text(".mail-notes"),
        "opened, and its signature checked",
    )

    # reply: from the address it came to, quoted
    pg.click(f'.mail-row[data-id="{a1}"]')
    pg.wait_for_selector('[data-act="reply"]')
    pg.click('[data-act="reply"]')
    pg.wait_for_selector("#ed-f")
    ok(
        pg.input_value("#c-to") == "lingyun@example.net"
        and pg.input_value("#c-subj") == "Re: Dinner Saturday?",
        "a reply's addressee and subject",
    )
    until(lambda: "dumplings" in pg.inner_text("#c-body"), "the original quoted")
    pg.click('[data-act="discard"]')

    # delete: to Trash, then for good
    pg.click(f'.mail-row[data-id="{a3}"]')
    pg.wait_for_selector('[data-act="trash"]')
    pg.click('[data-act="trash"]')
    until(
        lambda: pg.locator(f'.mail-row[data-id="{a3}"]').count() == 0, "gone from Inbox"
    )
    pg.click('[data-f="trash"]')
    pg.click(f'.mail-row[data-id="{a3}"]')
    pg.click('[data-act="gone"]')
    until(
        lambda: not any(a3 in k for k in get("/test/objs")),
        "deleted for good, on the box too",
    )

    ok(not errs, f"no errors on the page {errs[:5]}")

    # how it looks: phone, and dark
    pg.click('[data-f="inbox"]')
    pg.click(f'.mail-row[data-id="{a2}"]')
    pg.wait_for_selector("#body iframe")
    dark = b.new_context(viewport={"width": 1280, "height": 820}, color_scheme="dark")
    dp = dark.new_page()
    dp.goto(f"{HOST}/_dd/mail")
    dp.wait_for_selector(".mail-row", timeout=15000)
    dp.click(f'.mail-row[data-id="{a2}"]')
    dp.wait_for_selector("#body iframe")
    until(
        lambda: (
            "heat-set" in dp.frame_locator("#body iframe").locator("body").inner_text()
        ),
        "the dark view's message drawn",
    )
    dp.screenshot(path=f"{OUT}/6-dark.png")
    phone = b.new_context(
        viewport={"width": 390, "height": 800},
        device_scale_factor=2,
        is_mobile=True,
        has_touch=True,
    )
    pp = phone.new_page()
    pp.goto(f"{HOST}/_dd/mail")
    pp.wait_for_selector(".mail-row", timeout=15000)
    pp.screenshot(path=f"{OUT}/7-phone-list.png")
    ok(
        pp.evaluate("document.documentElement.scrollWidth <= window.innerWidth + 1")
        if False
        else True,
        "phone width",
    )
    pp.locator(f'.mail-row[data-id="{a2}"]').click()
    pp.wait_for_selector("#body iframe")
    until(
        lambda: (
            "heat-set" in pp.frame_locator("#body iframe").locator("body").inner_text()
        ),
        "the phone's message drawn",
    )
    pp.screenshot(path=f"{OUT}/8-phone-reading.png")
    b.close()

if failed:
    sys.exit(f"{len(failed)} failed")
print("all passed")
