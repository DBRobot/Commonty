# Every link in the bar and every tile on the home page opens in the tab it
# was clicked in, nowhere else: on a box's pages in a browser, on the pages
# the app serves a browser it let in (s-<label>.localhost), and in the bar
# above Photos and Passwords there (p- and v-). The pages, the bar and its
# script are the real ones (app/web/_dd, which the box serves the same);
# /_dd/me is a member's as the gate writes it (pages::me_json), with the
# services the roles add. Nothing answers but this script: every other
# address is a page that says it was reached.
import json
import os
import sys
from playwright.sync_api import sync_playwright

WEB = sys.argv[1]
DOMAIN = "commonty.test"
LABEL = "0123456789abcdef0123456789abcdef"
APP = f"http://s-{LABEL}.localhost:41660"
PHOTOS = f"http://p-{LABEL}.localhost:41661"
HOME = f"https://home.{DOMAIN}"

ICON = '<path d="M3 3h14v14H3z"/>'


def service(name, url):
    return {
        "name": name,
        "url": url,
        "icon": ICON,
        "blurb": name,
        "color": "#2f6fd6",
        "host": url.split("/")[2],
        "shut": False,
    }


ME = {
    "user": "tester",
    "initial": "t",
    "demo": False,
    "role": "member",
    "home": HOME,
    "menu": [
        [{"label": "Metrics", "url": f"https://metrics.{DOMAIN}/"}],
        [
            {"label": "Settings", "url": "/_dd/settings"},
            {"label": "Sign out", "url": "/_dd/logout"},
        ],
    ],
    "services": [
        service("Photos", f"https://photos.{DOMAIN}/_dd/photos"),
        service("Movies & TV", f"https://files.{DOMAIN}/_dd/media"),
        service("Files", f"https://files.{DOMAIN}/_dd/files"),
        service("Calendar", f"https://files.{DOMAIN}/_dd/calendar"),
        service("Contacts", f"https://files.{DOMAIN}/_dd/contacts"),
        service("Mail", f"https://files.{DOMAIN}/_dd/mail"),
        service("Passwords", f"https://vault.{DOMAIN}/"),
        service("Chat", f"https://llm.{DOMAIN}/"),
        service("Games", f"https://games.{DOMAIN}/"),
        service("Git", f"https://git.{DOMAIN}/"),
    ],
}

# the pages the app carries, by path (app/src/site.rs page_for)
PAGES = {
    "/_dd/home": "home",
    "/_dd/files": "files",
    "/_dd/media": "media",
    "/_dd/calendar": "calendar",
    "/_dd/contacts": "contacts",
    "/_dd/mail": "mail",
    "/_dd/settings": "settings",
    "/_dd/chat": "chat",
}
TYPES = {
    ".js": "text/javascript",
    ".css": "text/css",
    ".svg": "image/svg+xml",
    ".woff2": "font/woff2",
}


def answer(route):
    req = route.request
    u = req.url.split("#")[0]
    origin = "/".join(u.split("/")[:3])
    path = "/" + "/".join(u.split("/")[3:]).split("?")[0]
    host = origin.split("//")[-1].split(":")[0]
    ours = host == DOMAIN or host.endswith("." + DOMAIN) or origin in (APP, PHOTOS)
    if not ours:
        return route.abort()
    if path == "/_dd/me":
        return route.fulfill(
            status=200, content_type="application/json", body=json.dumps(ME)
        )
    if path.startswith("/_dd/static/"):
        f = os.path.join(WEB, "static", path[len("/_dd/static/") :])
        if os.path.isfile(f):
            return route.fulfill(
                status=200,
                content_type=TYPES.get(
                    os.path.splitext(f)[1], "application/octet-stream"
                ),
                body=open(f, "rb").read(),
            )
        return route.fulfill(status=404, body="")
    # the home page on a box, and every carried page where the app serves
    # them (with the domain the app writes into each)
    page = (
        "home"
        if path == "/_dd/home"
        else PAGES.get(path)
        if origin in (APP, PHOTOS)
        else None
    )
    if page and req.resource_type == "document":
        html = open(os.path.join(WEB, "pages", page + ".html")).read()
        if origin in (APP, PHOTOS):
            html = html.replace(
                "</head>", f'<meta name="dd-domain" content="{DOMAIN}"></head>', 1
            )
        return route.fulfill(status=200, content_type="text/html", body=html)
    if req.resource_type == "document":
        return route.fulfill(
            status=200,
            content_type="text/html",
            body="<!doctype html><title>reached</title><p>reached",
        )
    return route.fulfill(status=404, body="")


failed = []


def check(what, ok):
    print(("ok  " if ok else "FAIL") + " " + what)
    if not ok:
        failed.append(what)


# every link a member can click on the page: the bar (brand, services,
# Friends, Docs, the menu) and the tiles; by position, as each visit draws
# them again
LINKS = """() => [...document.querySelectorAll(
  'header.dd-bar a[href], main .services a[href]')]
  .filter((a) => !a.classList.contains('skip-to'))"""


def links(page):
    return page.evaluate(
        f"() => ({LINKS})().map((a) => ({{ text: a.textContent.trim().slice(0, 40), href: a.href, raw: a.getAttribute('href') }}))"
    )


def ready(page, url):
    page.goto(url)
    page.wait_for_selector("main .services a.service")
    page.wait_for_selector("header.dd-bar nav.links")
    page.wait_for_selector("header.dd-bar details.menu nav a", state="attached")
    page.wait_for_selector("header.dd-bar nav.links a.s", state="attached")


def where(page, start, i):
    """click link i of `start`; where this tab went, and how many tabs there are"""
    ready(page, start)
    before = page.url
    page.evaluate(f"(i) => ({LINKS})()[i].click()", i)
    try:
        page.wait_for_url(lambda u: u != before, timeout=5000)
        page.wait_for_load_state("domcontentloaded")
    except Exception:
        pass
    return page.url


with sync_playwright() as p:
    b = p.chromium.launch(args=["--mute-audio"])
    ctx = b.new_context()
    ctx.route("**/*", answer)
    opened = []
    ctx.on("page", lambda pg: opened.append(pg))
    page = ctx.new_page()
    opened.clear()

    def run(start):
        ready(page, start)
        found = links(page)
        names = [link["text"] for link in found]
        for want in ("Files", "Calendar", "Contacts", "Passwords", "Photos", "Games"):
            check(f"{start}: a link to {want}", any(want in n for n in names))
        for i, link in enumerate(found):
            check(
                f"{start}: {link['text']!r} goes somewhere ({link['raw']})",
                link["raw"] not in ("#", "", None),
            )
            n = len(ctx.pages)
            got = where(page, start, i)
            check(
                f"{start}: {link['text']!r} stays in this tab",
                len(ctx.pages) == n and not opened,
            )
            opened.clear()
            for extra in ctx.pages[1:]:
                extra.close()
            # sign out too: it posts to the address it shows
            want = link["href"]
            check(
                f"{start}: {link['text']!r} lands at {want}",
                got.split("#")[0] == want.split("#")[0],
            )
            if got.split("#")[0] != want.split("#")[0]:
                print("   it landed at", got)

    def tile(start, name):
        ready(page, start)
        return page.evaluate(
            "(n) => [...document.querySelectorAll('main .services a.service')].find((a) => a.querySelector('.name').textContent === n)?.href",
            name,
        )

    # 1. a box's pages in a browser: every link is the address it shows
    run(f"{HOME}/_dd/home")
    check(
        "box: Calendar's tile is the box's Calendar",
        tile(f"{HOME}/_dd/home", "Calendar") == f"https://files.{DOMAIN}/_dd/calendar",
    )
    check(
        "box: Passwords' tile is the vault's front",
        tile(f"{HOME}/_dd/home", "Passwords") == f"https://vault.{DOMAIN}/",
    )

    # 2. the app's pages in a browser it let in: its own pages here, Photos
    # and Passwords through its door for this browser, the rest the box's
    run(f"{APP}/_dd/home")
    check(
        "app: Calendar's tile is the app's Calendar",
        tile(f"{APP}/_dd/home", "Calendar") == f"{APP}/_dd/calendar",
    )
    check(
        "app: Contacts' tile is the app's Contacts",
        tile(f"{APP}/_dd/home", "Contacts") == f"{APP}/_dd/contacts",
    )
    check(
        "app: Passwords' tile is the app's door to the vault",
        tile(f"{APP}/_dd/home", "Passwords") == f"{APP}/_dd/app/go?service=vault",
    )
    check(
        "app: Photos' tile is the app's door to Photos",
        tile(f"{APP}/_dd/home", "Photos") == f"{APP}/_dd/app/go?service=photos",
    )
    # Calendar, clicked, is the app's own Calendar page
    ready(page, f"{APP}/_dd/home")
    page.click("main .services a.service:has-text('Calendar')")
    try:
        page.wait_for_url(f"{APP}/_dd/calendar", timeout=5000)
        page.wait_for_selector(
            "script[src$='/calendar.js']", state="attached", timeout=5000
        )
        check("app: Calendar opens the app's Calendar page", True)
    except Exception:
        check(
            f"app: Calendar opens the app's Calendar page (landed at {page.url})", False
        )

    # 3. the bar above Photos (and Passwords) the app serves: the site's
    # pages are the app's site server beside it
    run(f"{PHOTOS}/_dd/home")
    check(
        "photos: Files goes to the app's Files",
        tile(f"{PHOTOS}/_dd/home", "Files") == f"{APP}/_dd/files",
    )

    check("no tab was opened", len(ctx.pages) == 1)
    b.close()

if failed:
    print(f"{len(failed)} failed")
    sys.exit(1)
print("all passed")
