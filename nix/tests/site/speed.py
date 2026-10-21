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
import time

from stack import Watch

RUNS = 3
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
# from the click until it is drawn, in units of a fixed piece of page work
# timed in the same browser first (calibrate). A runner's load moves wall
# time five-fold (front page: 64 ms on a quiet laptop, 0.6-9 s on node1 at
# load 35-70); the CPU a page takes, in units, stays within about 30% of
# itself on either machine. Wall time is reported, and past HUNG_MS fails.
#
# What main measured, 2026-10-21 (units: highest of three runs on a quiet
# laptop and three on node1 at load 36-58; wall: quiet laptop median):
#
#   page          units laptop/node1   wall    KiB  requests
#   front page        1.29  1.75       64 ms   157     9
#   docs              0.71  0.53       53      380    19
#   sign-in           0.97  0.72       61      147    16
#   home              1.36  1.05       66      148    17
#   settings          2.02  1.57       96      216    34
#   friends           1.33  1.11       67      202    20
#   git               0.97  1.01       54      231    34
#   metrics           1.04  0.78      105      177    69
#
# A budget is the higher of the two x1.5 in units, KiB +10%, requests +3.
MEASURED = {
    # name: (units, KiB, requests)
    "front page": (1.75, 157, 9),
    "docs": (0.71, 380, 19),
    "sign-in": (0.97, 147, 16),
    "home": (1.36, 148, 17),
    "settings": (2.02, 216, 34),
    "friends": (1.33, 202, 20),
    "git": (1.01, 231, 34),
    "metrics": (1.04, 177, 69),
}
BUDGET = {k: (round(v[0] * 1.5, 1), int(v[1] * 1.10) + 1, v[2] + 3) for k, v in MEASURED.items()}

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
    page.wait_for_function(DRAWN, arg=sel)
    w.settled()
    wall = (time.perf_counter() - t0) * 1000
    c1 = cpu(cdp)
    # a page in a new renderer starts its count again
    return wall, (c1 - c0 if c1 >= c0 else c1)


CALIBRATE = """() => {
  // a fixed piece of what a page does: build a list, lay it out, read and
  // write JSON; the same work on every machine
  const root = document.createElement('main');
  document.body.replaceChildren(root);
  for (let i = 0; i < 3000; i++) {
    const li = document.createElement('div');
    li.className = 'row r' + (i % 7);
    li.textContent = 'row ' + i + ' ' + 'x'.repeat(i % 40);
    root.append(li);
  }
  let h = root.getBoundingClientRect().height;
  const data = Array.from({length: 4000}, (_, i) => ({i, name: 'n' + i, tags: ['a', 'b', String(i)]}));
  for (let k = 0; k < 5; k++) h += JSON.parse(JSON.stringify(data)).length;
  return h;
}"""


def calibrate(browser):
    """CPU ms of a fixed piece of page work in this browser on this machine:
    a page's CPU is budgeted in these, so a slower runner is not a slower page"""
    p = browser.new_page()
    cdp = p.context.new_cdp_session(p)
    cdp.send("Performance.enable")
    p.set_content("<!doctype html><title>calibrate</title><body></body>")
    runs = []
    for _ in range(7):
        c0 = cpu(cdp)
        p.evaluate(CALIBRATE)
        runs.append(cpu(cdp) - c0)
    p.close()
    return statistics.median(runs)


def measure(s, browser):
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
        runs = [click_to(p, w, url, cdp, sel) for _ in range(RUNS)]
        ctx.close()
        out[name] = (
            round(statistics.median(c for _, c in runs)),
            round(kib),
            asked,
            round(statistics.median(t for t, _ in runs)),
        )
    return out


def run(s, browser, report):
    t0 = time.monotonic()
    unit = calibrate(browser)
    print(f"calibration: {unit:.1f} ms CPU a unit")
    got = {
        k: (round(v[0] / unit, 2),) + v[1:] + (v[0],)
        for k, v in measure(s, browser).items()
    }
    bad = []
    for name, (ms, kib, asked, wall, raw) in got.items():
        b = BUDGET.get(name)
        line = f"{name:12} {ms:5} units CPU ({raw} ms) {kib:5} KiB {asked:4} requests {wall:6} ms wall"
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
