# The journeys, in a browser on the box (site-journeys.nix): for each web
# service, the one thing it is for, from signing in to signing out, against
# the real services. Each journey waits on what the page shows, never on a
# clock, and records how long it took.
#
#   journeys.py <domain> <app.py> <carried> <web> <app state> <out.json> <picture>
import hashlib
import json
import os
import re
import ssl
import subprocess
import sys
import time
import urllib.parse
import urllib.request

from playwright.sync_api import expect, sync_playwright

D, APP, CARRIED, WEB, STATE, OUT, PICTURE = sys.argv[1:8]
APP_PORT = 8790
# the name the app gives a browser it lets in (shell.js servedByApp):
# s-<32 hex>.localhost, which a browser keeps on this computer
APPURL = f"http://s-{'0' * 32}.localhost:{APP_PORT}"
TLS = ssl.create_default_context(cafile=os.environ["SITE_CA"])
T = 30_000  # ms any one thing on a page may take before the journey fails


def url(host, path="/"):
    return f"https://{host}.{D}{path}"


def sign_in(token):
    """the app's sign-in: its device token for a session on the box"""
    req = urllib.request.Request(
        url("home", "/_dd/app/signin"),
        data=urllib.parse.urlencode({"token": token}).encode(),
        method="POST",
        headers={
            # the app signs in from its own window
            "origin": "commonty://localhost",
            "content-type": "application/x-www-form-urlencoded",
        },
    )

    class Keep(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):
            return None

    op = urllib.request.build_opener(Keep, urllib.request.HTTPSHandler(context=TLS))
    try:
        r = op.open(req)
    except urllib.error.HTTPError as e:
        r = e
    for k, v in r.headers.items():
        if k.lower() == "set-cookie" and v.startswith("dd_session="):
            return v.split(";")[0].split("=", 1)[1]
    raise AssertionError(
        f"no session from /_dd/app/signin ({r.status}): {r.read()[:200]!r}"
    )


results = []


def journey(name):
    def wrap(f):
        def run(*a):
            t0 = time.monotonic()
            try:
                detail = f(*a) or ""
                results.append(
                    {
                        "journey": name,
                        "ok": True,
                        "ms": round((time.monotonic() - t0) * 1000),
                        "detail": detail,
                    }
                )
                print(f"ok   {name:14} {results[-1]['ms']:6} ms  {detail}", flush=True)
            except Exception as e:
                page = a[0]
                try:
                    text = page.locator("body").inner_text(timeout=2000)[:600]
                except Exception:
                    text = "?"
                print(f"     at {page.url}: {text!r}", flush=True)
                for line in console[-15:]:
                    print(f"     console: {line}", flush=True)
                results.append(
                    {
                        "journey": name,
                        "ok": False,
                        "ms": round((time.monotonic() - t0) * 1000),
                        "detail": str(e)[:2000],
                    }
                )
                print(f"FAIL {name:14} {e}", flush=True)

        return run

    return wrap


@journey("settings")
def settings(page):
    page.goto(url("home", "/_dd/settings"))
    expect(page.locator("#name")).to_have_text("mia", timeout=T)
    return "her name on the Account section"


@journey("devices")
def devices(page):
    page.goto(url("home", "/_dd/settings#devices"))
    pane = page.locator('[data-pane="devices"]')
    expect(pane).to_be_visible(timeout=T)
    # the device her session came from, listed from the gate's registry
    # the session she is signed in with, from the gate's registry
    expect(pane.locator("#dv-sessions li").first).to_be_visible(timeout=T)
    return "the Devices tab lists her sign-ins"


@journey("backups")
def backups(page):
    page.goto(url("home", "/_dd/settings#backups"))
    pane = page.locator('[data-pane="backups"]')
    expect(pane).to_be_visible(timeout=T)
    expect(pane.locator("h2").first).to_be_visible(timeout=T)
    return "the Backups tab opens"


@journey("keys in a browser")
def keys_in_browser(page):
    """the parts of Settings that hold a key, in a plain browser: on the
    box's page each is one button that hands it to the app (shell.js
    throughApp), and says so plainly when no app takes it; the app's copy
    in the same browser opens them"""
    nothing = "Nothing opened: the Commonty app"
    for tab, button in (
        ("backups", '[data-pane="backups"] button:has-text("Open Backups")'),
        ("devices", "#dv-show"),
        ("leave", "#lv-files-go"),
    ):
        page.goto(url("home", f"/_dd/settings#{tab}"))
        pane = page.locator(f'[data-pane="{tab}"]')
        expect(pane).to_be_visible(timeout=T)
        b = page.locator(button).first
        expect(b).to_be_enabled(timeout=T)
        if tab == "devices":
            # the box hands this to the app once it knows it is no demo
            page.wait_for_load_state("networkidle")
        b.click()
        # no app on this computer: the page says so, with the download
        expect(pane.get_by_text(nothing).first).to_be_visible(timeout=T)
        expect(
            pane.locator(
                'a[href="/_dd/download"]', has_text="Get the Commonty app"
            ).first
        ).to_be_visible(timeout=T)
        expect(page).to_have_url(url("home", f"/_dd/settings#{tab}"))
    # the app's copy of Settings, in the same browser: the names open
    page.goto(APPURL + "/_dd/settings#devices")
    show = page.locator("#dv-show")
    expect(show).to_be_enabled(timeout=T)
    show.click()
    expect(show).to_be_enabled(timeout=T)
    page.wait_for_load_state("networkidle")
    msg = page.locator("#dv-msg").inner_text()
    assert not msg.strip(), f"Show names in the app's copy: {msg!r}"
    # and the Leave tab's downloads are live there, not handed on
    page.goto(APPURL + "/_dd/settings#leave")
    expect(page.locator("#lv-take-box")).to_be_visible(timeout=T)
    expect(page.locator("#lv-not-app")).to_be_hidden(timeout=T)
    expect(page.locator("#lv-files-go")).to_be_enabled(timeout=T)
    return "disk images, device names and Leave's downloads hand off to the app, and open in its copy"


@journey("docs")
def docs(page):
    page.goto(url("home", "/_dd/home"))
    expect(page.locator("#services li").first).to_be_visible(timeout=T)
    page.keyboard.press("/")
    page.keyboard.type("calendar")
    found = page.locator(".dd-search a", has_text="Calendar and contacts").first
    expect(found).to_be_visible(timeout=T)
    href = found.get_attribute("href") or ""
    assert "calendar" in href, href
    # the page it names, as the docs serve it on this box
    page.goto(url("home", "/_dd/docs/page/dev/calendar"))
    expect(page.locator(".docs-md h1")).to_have_text("Calendar and contacts", timeout=T)
    return f"search found {href}"


@journey("git")
def git(page):
    page.goto(url("git", "/"))
    page.locator("a", has_text="notes").first.click(timeout=T)
    expect(page).to_have_url(url("git", "/mia/notes"), timeout=T)
    expect(page.get_by_text("where the otters are written down").first).to_be_visible(
        timeout=T
    )
    return "her repository, from the forge"


@journey("metrics")
def metrics(page):
    page.goto(url("metrics", "/"))
    card = page.locator("#app a", has_text="box").first
    expect(card).to_be_visible(timeout=T)
    card.click()
    expect(page).to_have_url(re.compile(r"/box"), timeout=T)
    expect(page.locator("#app")).to_contain_text("box", timeout=T)
    return "the box's own page, from Thanos"


@journey("games")
def games(page):
    page.goto(url("games", "/"))
    expect(page.get_by_text("Probe").first).to_be_visible(timeout=T)
    return "the catalogue, from the games manager"


@journey("chat")
def chat(page):
    page.goto(APPURL + "/_dd/chat")
    expect(page.locator("#ta")).to_be_editable(timeout=T)
    page.fill("#ta", "Tell me about otters")
    page.click("#send")
    expect(page.locator("#log")).to_contain_text("Otters hold hands.", timeout=T)
    return "an answer from the model, through nginx and the gate"


@journey("files")
def files(page):
    page.goto(APPURL + "/_dd/files")
    expect(page.locator("#drive")).to_be_visible(timeout=T)
    row = page.locator("#rows tr", has_text="otters.txt").first
    expect(row).to_be_visible(timeout=T)
    row.locator("td").nth(1).click()
    expect(page.locator("#pv-name")).to_have_text("otters.txt", timeout=T)
    expect(page.locator("#preview")).to_contain_text(
        "Otters hold hands while they sleep.", timeout=T
    )
    return "a file dd put in, opened in the page"


@journey("calendar")
def calendar(page):
    page.goto(APPURL + "/_dd/calendar")
    expect(page.locator("#new-event")).to_be_enabled(timeout=T)
    page.click("#new-event")
    page.fill("#f-title", "Otter count")
    page.click("#edit-ok")
    expect(page.locator("#grid, #cal").first).to_contain_text("Otter count", timeout=T)
    return "an event, saved to her library"


@journey("contacts")
def contacts(page):
    page.goto(APPURL + "/_dd/contacts")
    expect(page.locator("#new-person")).to_be_enabled(timeout=T)
    page.click("#new-person")
    page.fill("#p-name", "Ada Otter")
    page.click("#edit-ok")
    expect(page.locator("#plist")).to_contain_text("Ada Otter", timeout=T)
    return "a contact, saved to her library"


@journey("media")
def media(page):
    page.goto(APPURL + "/_dd/media")
    title = page.locator("#films").get_by_text("River Otters").first
    expect(title).to_be_visible(timeout=T)
    title.click()
    expect(page.locator("#r-title")).to_contain_text("River Otters", timeout=T)
    return "a title from her library's Movies"


@journey("photos")
def photos(page, picture):
    """the demo's gallery, from museum through the gate: a photo opened, and
    what the gate hands the browser is the picture that was saved"""
    page.goto(url("home", "/_dd/demo"))
    page.goto(url("home", "/_dd/photos/demo"))
    pic = page.locator('button[aria-label="otter.jpg"]').first
    expect(pic).to_be_visible(timeout=T)
    pic.click()
    expect(page.locator("#view")).to_be_visible(timeout=T)
    expect(page.locator("#cap")).to_contain_text("otter.jpg", timeout=T)
    # the whole picture, drawn
    page.wait_for_function(
        "() => { const b = document.getElementById('big'); return b.src.includes('/photo/') && b.complete && b.naturalWidth === 64; }",
        timeout=T,
    )
    got = page.evaluate(
        """async () => {
          const b = await (await fetch(document.getElementById('big').src)).arrayBuffer();
          const h = await crypto.subtle.digest('SHA-256', b);
          return [...new Uint8Array(h)].map((x) => x.toString(16).padStart(2, '0')).join('');
        }"""
    )
    want = hashlib.sha256(open(picture, "rb").read()).hexdigest()
    assert got == want, f"the picture shown is not the one saved: {got} != {want}"
    return "a photo of the demo's, opened whole"


@journey("sign out")
def sign_out(page, ctx):
    page.goto(url("home", "/_dd/logout"))
    page.locator("button", has_text="Sign out").first.click(timeout=T)
    page.wait_for_url(re.compile(r"^(?!.*/_dd/logout)"), timeout=T)
    r = ctx.request.get(url("home", "/_dd/me"), fail_on_status_code=False)
    assert r.status == 401, f"/_dd/me after signing out: {r.status}"
    page.goto(url("home", "/_dd/settings"))
    expect(page).to_have_url(re.compile(r"/_dd/login"), timeout=T)
    return "the session ended at the gate"


errors = []
console = []


def main():
    st = json.load(open(STATE))
    app = subprocess.Popen(
        [sys.executable, APP, str(APP_PORT), CARRIED, WEB, D, STATE],
        stdout=open("/tmp/app.log", "w"),
        stderr=subprocess.STDOUT,
    )
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch(args=["--mute-audio"])
            t0 = time.monotonic()
            session = sign_in(st["token"])
            results.append(
                {
                    "journey": "sign in",
                    "ok": True,
                    "ms": round((time.monotonic() - t0) * 1000),
                    "detail": "a session from the device's token",
                }
            )
            # the box's certificate is the test's own
            ctx = browser.new_context(
                ignore_https_errors=True, viewport={"width": 1280, "height": 900}
            )
            ctx.add_cookies(
                [
                    {
                        "name": "dd_session",
                        "value": session,
                        "domain": "." + D,
                        "path": "/",
                        "secure": True,
                        "httpOnly": True,
                        "sameSite": "Lax",
                    }
                ]
            )
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.on("console", lambda m: console.append(f"{m.type}: {m.text[:300]}"))
            page.on(
                "response",
                lambda r: (
                    console.append(f"http {r.status} {r.request.method} {r.url[:150]}")
                    if r.status >= 400
                    else None
                ),
            )
            for j in (
                settings,
                devices,
                backups,
                keys_in_browser,
                docs,
                git,
                metrics,
                games,
                chat,
                files,
                calendar,
                contacts,
                media,
            ):
                j(page)
            sign_out(page, ctx)
            # the demo, in a browser of its own
            demo = browser.new_context(ignore_https_errors=True)
            photos(demo.new_page(), PICTURE)
            browser.close()
    finally:
        app.terminate()
        app.wait()
    json.dump(results, open(OUT, "w"), indent=1)
    bad = [r for r in results if not r["ok"]]
    print(f"site-journeys: {len(results) - len(bad)} of {len(results)} journeys")
    if errors:
        print("page errors:", errors[:10])
    sys.exit(1 if bad else 0)


main()
