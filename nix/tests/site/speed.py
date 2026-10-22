# The speed budget (checks.site-speed): for each main page, the time from a
# click on a link to it until it is drawn, and the bytes a first visit
# brings down, in Chromium against the gateway box's real nginx and gate
# (stack.py). A page over its budget fails. The budgets sit a little above
# what main measured (BUDGET below says when and what), so a regression
# fails and an ordinary run does not.
#
# "Drawn": the bar has the person (from /_dd/me) where the page has one, its
# <main> holds text, and nothing the page asked for is still out. Time is
# the median of RUNS clicks on a warm page, as a member moves about; bytes
# are one cold visit, nothing cached.
import json
import statistics
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from stack import Watch

# seven: one click in a few lands on a burst of the runner's other work
RUNS = 7
# past this, a page has hung, whatever the runner's load (wall time on a
# runner moves five-fold with its load, so it is a ceiling, not a budget)
HUNG_MS = 15000

# (name, person, host, path, what is there once it is drawn: the page's own
# content, filled from what it asked for)
PAGES = [
    ("front page", None, "@", "/", "main section.idea h2"),
    ("docs", None, "@", "/docs", "#docs-body .docs-md h1"),
    ("sign-in", None, "home", "/_dd/login", "#go"),
    ("home", "mia", "home", "/_dd/home", "#services li a.service"),
    ("settings", "mia", "home", "/_dd/settings", "#name:not(:empty)"),
    ("friends", "mia", "home", "/_dd/friends", "#people li"),
    ("git", "mia", "git", "/", "#app h1"),
    ("metrics", "david", "metrics", "/", "#app .cards"),
]

# Time is budgeted as the page's own work: the CPU its main thread spends
# from the click until it is drawn, in units of the same click on a fixed
# reference page the test serves itself (REF_*: a document, its stylesheet
# and module, twenty fetches and a list drawn from them). A reference click
# goes just before each of a page's clicks, and a page's units are the
# median of its clicks over theirs. A runner's load moves wall time
# five-fold, and CPU time with it; a page and the reference move together.
#
# The unit was a loop of DOM and JSON work timed once at the start, until
# 2026-10-22: a page is mostly navigation, style and fetches, and the loop
# ran twice as fast against them on node2 (Ryzen 7520U) as on node1 (i7
# 1165G7 under load), so the same page measured 0.65 units on node1 and
# 1.3-1.6 on node2, and metrics' budget, taken on the laptop and node1
# only, failed on node2. One timing at the start also missed the load
# moving under the run (node1: 30 ms a unit to 88 within a run).
#
# What main measured, 2026-10-22 (units: highest of three runs each on a
# quiet laptop, node1 at load 18-27 and node2 at load 7-15; wall: quiet
# laptop median; KiB and requests as 2026-10-21):
#
#   page          units laptop/node1/node2   wall    KiB  requests
#   front page        1.05  1.24  1.29       62 ms   157     9
#   docs              0.55  0.52  0.52       59      380    19
#   sign-in           0.75  0.73  0.81       58      147    16
#   home              1.07  1.06  1.20       63      148    17
#   settings          1.63  1.55  1.75       80      216    34
#   friends           1.09  1.05  1.20       63      202    20
#   git               0.94  1.05  0.97       53      231    34
#   metrics           0.93  0.78  1.18       94      177    69
#
# A budget is the highest of the three x1.5 in units, KiB +10%, requests +3.
MEASURED = {
    # name: (units, KiB, requests)
    "front page": (1.29, 157, 9),
    "docs": (0.55, 380, 19),
    "sign-in": (0.81, 147, 16),
    "home": (1.20, 148, 17),
    "settings": (1.75, 216, 34),
    "friends": (1.20, 202, 20),
    "git": (1.05, 231, 34),
    "metrics": (1.18, 177, 69),
}
BUDGET = {
    k: (round(v[0] * 1.5, 1), int(v[1] * 1.10) + 1, v[2] + 3)
    for k, v in MEASURED.items()
}

DRAWN = """(sel) => {
  const me = document.getElementById('me');
  if (me && me.childElementCount === 0) return false;
  return !!document.querySelector(sel);
}"""


def cpu(cdp):
    """the renderer's main thread, CPU ms so far (Chrome's ThreadTime)"""
    m = {x["name"]: x["value"] for x in cdp.send("Performance.getMetrics")["metrics"]}
    return m["ThreadTime"] * 1000


def click_to(page, w, url, cdp, sel):
    """from the page as it is, a click on a link to url, until url is drawn:
    (wall ms, main-thread CPU ms)"""
    page.evaluate(
        "(u) => { const a = document.createElement('a'); a.href = u; a.id = 'speed-link'; a.textContent = 'go'; document.body.prepend(a); }",
        url,
    )
    c0 = cpu(cdp)
    t0 = time.perf_counter()
    page.click("#speed-link")
    page.wait_for_url(url.split("#")[0] + "**")
    # by a timer, not each frame: a frame asked for is the page's CPU, and
    # more frames pass as the runner's load stretches the wait
    page.wait_for_function(DRAWN, arg=sel, polling=20)
    w.settled()
    wall = (time.perf_counter() - t0) * 1000
    c1 = cpu(cdp)
    # a page in a new renderer starts its count again
    return wall, (c1 - c0 if c1 >= c0 else c1)


REF_CSS = "".join(
    f".r{i} {{ color: rgb({i}, {i * 3 % 255}, {i * 7 % 255}); padding: {i % 5}px; }}\n"
    f".c{i} > span + b {{ margin-left: {i % 9}px; }}\n"
    for i in range(150)
)
REF_JS = """
const rs = await Promise.all(Array.from({length: 20}, (_, i) => fetch('/d/' + i).then(r => r.json())));
const ul = document.createElement('ul');
for (const d of rs) for (const it of d.items) {
  const li = document.createElement('li');
  li.className = 'r' + (it.i % 150) + ' c' + (it.i % 150);
  const s = document.createElement('span');
  s.textContent = it.name;
  const b = document.createElement('b');
  b.textContent = it.tags.join(', ');
  li.append(s, b);
  ul.append(li);
}
document.querySelector('main').replaceChildren(ul);
document.body.dataset.drawn = '1';
"""
REF_HTML = """<!doctype html><html lang="en"><head><meta charset="utf-8"><title>reference</title>
<link rel="stylesheet" href="/ref.css"></head><body><header><nav><a href="/a">a</a> <a href="/b">b</a></nav></header>
<main><p>Loading</p></main><script type="module" src="/ref.js"></script></body></html>"""


def ref_data(i):
    items = [
        {"i": i * 15 + j, "name": f"item {i}-{j}", "tags": ["a", "b", str(j)]}
        for j in range(15)
    ]
    return json.dumps({"items": items})


class RefHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_GET(self):
        # the document and its data each time, its stylesheet and module
        # from the cache, as a warm page of ours
        if self.path == "/ref.css":
            body, kind, cache = REF_CSS, "text/css", "max-age=3600"
        elif self.path == "/ref.js":
            body, kind, cache = REF_JS, "text/javascript", "max-age=3600"
        elif self.path.startswith("/d/"):
            body, kind, cache = (
                ref_data(int(self.path[3:])),
                "application/json",
                "no-store",
            )
        else:
            body, kind, cache = REF_HTML, "text/html", "no-store"
        body = body.encode()
        self.send_response(200)
        self.send_header("content-type", kind)
        self.send_header("cache-control", cache)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class Reference:
    """the reference page in a tab of its own, and a click on it timed"""

    def __init__(self, browser):
        self.srv = ThreadingHTTPServer(("127.0.0.1", 0), RefHandler)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.srv.server_address[1]}/ref.html"
        self.ctx = browser.new_context()
        self.page = self.ctx.new_page()
        self.cdp = self.ctx.new_cdp_session(self.page)
        self.cdp.send("Performance.enable")
        self.page.goto(self.url)
        self.page.wait_for_function("() => document.body.dataset.drawn", polling=20)
        for _ in range(3):  # its code compiled and cached, as a warm page's
            self.click()

    def click(self):
        """main-thread CPU ms from a click on a link to it until it is drawn"""
        p = self.page
        p.evaluate(
            "(u) => { const a = document.createElement('a'); a.href = u; a.id = 'speed-link'; a.textContent = 'go'; document.body.prepend(a); window.speedOld = 1; }",
            self.url,
        )
        c0 = cpu(self.cdp)
        p.click("#speed-link")
        p.wait_for_function(
            "() => !window.speedOld && document.body?.dataset.drawn", polling=20
        )
        c1 = cpu(self.cdp)
        return c1 - c0 if c1 >= c0 else c1

    def close(self):
        self.ctx.close()
        self.srv.shutdown()


def measure(s, browser, ref):
    out = {}
    for name, who, host, path, sel in PAGES:
        url = s.url(host, path)
        # bytes: a first visit, cold
        ctx = s.browser_context(browser, who=who)
        p = ctx.new_page()
        w = Watch(p)
        p.goto(url)
        p.wait_for_function(DRAWN, arg=sel)
        w.settled()
        kib = w.bytes / 1024
        asked = w.count
        ctx.close()
        # time: from a page of the same name, warm, as a member clicks on
        ctx = s.browser_context(browser, who=who)
        p = ctx.new_page()
        w = Watch(p)
        cdp = ctx.new_cdp_session(p)
        cdp.send("Performance.enable")
        p.goto(url)
        p.wait_for_function(DRAWN, arg=sel)
        w.settled()
        runs = []
        for _ in range(RUNS):
            unit = ref.click()
            wall, used = click_to(p, w, url, cdp, sel)
            runs.append((used / unit, used, unit, wall))
        ctx.close()
        out[name] = (
            round(statistics.median(r[0] for r in runs), 2),
            round(kib),
            asked,
            round(statistics.median(r[3] for r in runs)),
            round(statistics.median(r[1] for r in runs)),
            round(statistics.median(r[2] for r in runs)),
            sorted(round(r[0], 2) for r in runs),
        )
    return out


def run(s, browser, report):
    t0 = time.monotonic()
    ref = Reference(browser)
    got = measure(s, browser, ref)
    ref.close()
    bad = []
    for name, (ms, kib, asked, wall, raw, unit, each) in got.items():
        b = BUDGET.get(name)
        line = f"{name:12} {ms:5} units CPU ({raw} ms, unit {unit} ms) {kib:5} KiB {asked:4} requests {wall:6} ms wall"
        if wall > HUNG_MS:
            bad.append(f"{name}: {wall} ms from click to drawn: hung")
        if b:
            line += f"   budget {b[0]} units {b[1]} KiB {b[2]} requests"
            if ms > b[0]:
                bad.append(
                    f"{name}: {ms} units of the page's CPU from click to drawn, over its {b[0]}"
                )
            if kib > b[1]:
                bad.append(f"{name}: {kib} KiB on a first visit, over its {b[1]} KiB")
            if asked > b[2]:
                bad.append(
                    f"{name}: {asked} requests on a first visit, over its {b[2]}"
                )
        else:
            bad.append(
                f"{name}: no budget; measured {ms} ms, {kib} KiB, {asked} requests"
            )
        print(line)
        print(f"{'':12} each click: {each}")
    for name in set(BUDGET) - set(got):
        bad.append(f"{name}: has a budget and is not measured")
    json.dump(got, open(report, "w"), indent=1)
    print(f"site-speed: {len(got)} pages in {time.monotonic() - t0:.1f}s")
    for b in bad:
        print("FAIL", b)
    return not bad


if __name__ == "__main__":
    from playwright.sync_api import sync_playwright
    from stack import Stack

    out, conf, nginx, verify, env = sys.argv[1:6]
    with Stack(out + "/stack", conf, nginx, verify, env) as s, sync_playwright() as pw:
        browser = pw.chromium.launch(args=s.chrome_args())
        ok = run(s, browser, out + "/speed.json")
        browser.close()
    sys.exit(0 if ok else 1)
