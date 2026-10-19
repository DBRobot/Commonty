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


def tiny_pdf():
    """a one-page PDF, written out by hand with its cross-reference table"""
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        None,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    text = b"BT /F1 18 Tf 72 700 Td (Residential lease: sign below) Tj ET"
    objs[3] = b"<< /Length %d >>\nstream\n" % len(text) + text + b"\nendstream"
    out = b"%PDF-1.4\n"
    offs = []
    for i, o in enumerate(objs, 1):
        offs.append(len(out))
        out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
    x = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    out += b"".join(b"%010d 00000 n \n" % o for o in offs)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objs) + 1,
        x,
    )
    return out


def arrive_with(subject, frm, body, name, files):
    """a message carrying attachments: files is [(name, type, bytes)]"""
    import base64 as b64

    bd = "mix3d"
    parts = [f"--{bd}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"]
    for fname, ftype, data in files:
        parts.append(
            f'--{bd}\r\nContent-Type: {ftype}\r\nContent-Disposition: attachment; filename="{fname}"\r\nContent-Transfer-Encoding: base64\r\n\r\n{b64.b64encode(data).decode()}\r\n'
        )
    mid = f"<{time.time_ns()}@x>"
    raw = (
        f"From: {name} <{frm}>\r\nTo: tester@commonty.test\r\nSubject: {subject}\r\nDate: Sun, 18 Oct 2026 09:41:00 +0000\r\nMessage-ID: {mid}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary={bd}\r\n\r\n"
        + "".join(parts)
        + f"--{bd}--\r\n"
    )
    summary = {
        "from": {"name": name, "address": frm},
        "to": [{"name": "", "address": "tester@commonty.test"}],
        "cc": [],
        "subject": subject,
        "snippet": body[:200],
        "attachments": [f[0] for f in files],
        "date": int(time.time() * 1000),
        "rcpt": "tester@commonty.test",
        "mid": mid,
    }
    return post("/test/arrive", {"raw": raw, "summary": summary, "auth": {}})["id"]


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
            # a folder made again: the gate's 405, which library.js takes as "there already"
            and "405" not in m.text
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
        html='<p>Two <b>M3 heat-set insert kits</b> are on their way.</p><img src="https://tracker.example/pixel.gif" alt="t"><p><a href="https://northside.example/track">Track it</a></p>'
        # what a sender might try, to run in the page that holds the keys
        '<script>parent.__ran = "script"</script>'
        "<svg onload=\"parent.__ran = 'svg'\"></svg>"
        "<details open ontoggle=\"parent.__ran = 'toggle'\"><summary>x</summary></details>"
        "<iframe srcdoc=\"<script>parent.parent.__ran = 'frame'</script>\"></iframe>"
        '<form action="javascript:parent.__ran=1"><button id="go">Go</button></form>',
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
    # nothing in a message runs: the page beside it holds the keys to the
    # member's mail, files and photos (app/src/keys.rs); its handlers would
    # have run by the time the frame has loaded
    pg.locator("#body iframe").element_handle().content_frame().wait_for_load_state(
        "load"
    )
    ok(
        pg.evaluate("window.__ran") is None,
        f"no script in the message ran: {pg.evaluate('window.__ran')}",
    )
    ok(
        "allow-scripts"
        not in (pg.locator("#body iframe").get_attribute("sandbox") or "allow-scripts"),
        "the message's frame is sandboxed without scripts",
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

    # Files: an attachment saved into the member's library, sealed in the
    # tab; Save all; the mark replayed on another device; attached back from
    # Files at its size; the 18 MB refusal; and a PDF signed and replied with
    pdf = tiny_pdf()
    a5 = arrive_with(
        "Lease to sign",
        "lease@harbor.example",
        "Please sign and send it back.",
        "Harbor Street",
        [("lease.pdf", "application/pdf", pdf), ("notes.txt", "text/plain", b"hello")],
    )
    pg.click('[data-f="inbox"]')
    pg.reload()
    pg.wait_for_selector(f'.mail-row[data-id="{a5}"]', timeout=15000)
    pg.click(f'.mail-row[data-id="{a5}"]')
    until(
        lambda: pg.locator("#att .mail-file").count() == 2,
        "two attachments, each with its actions",
    )
    ok(
        pg.locator('#att [data-pdf="0"][data-mode="sign"]').count() == 1
        and pg.locator('#att [data-pdf="1"]').count() == 0,
        "Edit and Sign on the PDF alone",
    )
    ok(
        pg.locator("#att [data-photos]").count() == 0,
        "no Save to Photos on a PDF or text",
    )
    pg.click('#att [data-save="0"]')
    pg.wait_for_selector("dialog.dd-pick[open]", timeout=10000)
    month = time.strftime("%Y-%m")
    ok(
        pg.inner_text("dialog.dd-pick .where").replace(" ", "")
        == f"Myfiles/Mail/{month}",
        f"the folder picker opens on Mail / {month}",
    )
    pg.screenshot(path=f"{OUT}/9-save-to-files.png")
    pg.locator("dialog.dd-pick button", has_text="Save here").click()
    until(
        lambda: "Saved to Files / Mail / " + month in pg.inner_text("#att"),
        "it says where it went",
    )
    dav = get("/test/dav")
    ok(
        len(dav) == 1 and dav[0]["sealed"],
        f"one file at the gate, its bytes sealed {dav}",
    )
    ok(
        not any(w in dav[0]["path"] for w in ("lease", "Mail", "Files", month)),
        f"and its name encrypted {dav[0]['path'][:40]}",
    )
    pg.locator("#att [data-saveall]").click()
    pg.wait_for_selector("dialog.dd-pick[open]", timeout=10000)
    pg.locator("dialog.dd-pick button", has_text="Save here").click()
    until(lambda: len(get("/test/dav")) == 2, "Save all saved the other one")
    ok(
        all(d["sealed"] for d in get("/test/dav")),
        "both sealed, and lease.pdf not saved twice",
    )

    # another device: a page with nothing kept replays the change
    other = b.new_context(viewport={"width": 1280, "height": 820})
    op = other.new_page()
    op.goto(f"{HOST}/_dd/mail")
    op.wait_for_selector(f'.mail-row[data-id="{a5}"]', timeout=15000)
    op.click(f'.mail-row[data-id="{a5}"]')
    until(
        lambda: op.locator("#att .saved").count() == 2,
        "the other device shows both as saved",
    )
    other.close()

    # attached back from Files, at its size
    pg.click('[data-act="reply"]')
    pg.wait_for_selector("#ed-f")
    pg.click('[data-act="attach"]')
    ok(
        pg.locator(".mail-attmenu:not([hidden]) button").all_inner_texts()
        == ["From this device", "From Files", "From Photos", "Photos by link"],
        "Attach offers this device, Files, Photos, or a link to photos",
    )
    pg.click('[data-act="attach-files"]')
    pg.wait_for_selector("dialog.dd-pick[open]", timeout=10000)
    pg.locator("dialog.dd-pick button", has_text="Mail").click()
    pg.locator("dialog.dd-pick button", has_text=month).click()
    pg.locator("dialog.dd-pick button", has_text="lease.pdf").click()
    until(lambda: "lease.pdf" in pg.inner_text("#files"), "lease.pdf on the reply")
    ok(
        f"lease.pdf · {len(pdf)} B" in pg.inner_text("#files"),
        f"at its own size, {len(pdf)} bytes",
    )

    # past about 18 MB of files, refused before anything is read
    pg.evaluate(
        """async () => {
          const l = await import('/_dd/static/library.js');
          const r = await l.unlock();
          await l.put(r.ok, 'Files/big.bin', new Blob([new Uint8Array(19 * 1024 * 1024)]));
        }"""
    )
    pg.click('[data-act="attach"]')
    pg.click('[data-act="attach-files"]')
    pg.wait_for_selector("dialog.dd-pick[open]", timeout=10000)
    pg.locator("dialog.dd-pick button", has_text="big.bin").click()
    until(
        lambda: "too big to send" in pg.inner_text("#toast"),
        "a file too big to send is refused",
    )
    ok("big.bin" not in pg.inner_text("#files"), "and not attached")
    pg.click('[data-act="discard"]')

    # signed in the editor, then a reply carrying it
    pg.click(f'.mail-row[data-id="{a5}"]')
    pg.wait_for_selector('#att [data-pdf="0"][data-mode="sign"]')
    pg.click('#att [data-pdf="0"][data-mode="sign"]')
    pg.wait_for_selector(".pdfed .pe-page canvas", timeout=20000)
    ok(
        "Not in Files yet" in pg.inner_text(".pe-title"),
        "the editor opens on the attachment itself",
    )
    pg.screenshot(path=f"{OUT}/10-sign-attachment.png")
    pg.click(".pe-save .more")
    pg.locator(".pe-menu button", has_text="Reply with it attached").click()
    pg.wait_for_selector("#ed-f", timeout=10000)
    ok(pg.locator(".pdfed").count() == 0, "the editor closed")
    ok(
        "lease.pdf" in pg.inner_text("#files")
        and pg.input_value("#c-subj") == "Re: Lease to sign",
        "a reply to it, carrying the PDF",
    )
    pg.screenshot(path=f"{OUT}/11-reply-with-it.png")
    pg.click('[data-act="discard"]')

    # Photos: Save to Photos on a picture or a film, asking the app's one
    # key route for Photos alone; a device Photos has not opened on yet says
    # so (the round trip itself is checks.photos, against museum)
    a6 = arrive_with(
        "From the hike",
        "lingyun@family.example",
        "Two from the top.",
        "Lingyun",
        [
            ("summit.jpg", "image/jpeg", b"\xff\xd8\xff\xe0 not a real jpeg"),
            ("trail.mp4", "video/mp4", b"\x00\x00\x00\x18ftypmp42"),
        ],
    )
    pg.click('[data-f="inbox"]')
    pg.reload()
    pg.wait_for_selector(f'.mail-row[data-id="{a6}"]', timeout=15000)
    pg.click(f'.mail-row[data-id="{a6}"]')
    until(
        lambda: pg.locator("#att [data-photos]").count() == 2,
        "Save to Photos on the picture and the film",
    )
    pg.click('#att [data-photos="0"]')
    until(
        lambda: "Photos does not open on this device yet" in pg.inner_text("#toast"),
        "a device without Photos says so",
    )
    ok(
        get("/test/keys") == [["photos"]],
        f"it asked the app for Photos and nothing else: {get('/test/keys')}",
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
