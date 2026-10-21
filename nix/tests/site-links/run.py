# checks.site-links: the site's links, crawled as people see them each way
# their browser can find the box (crawl.py over site/stack.py), and the
# app's own pages read for where their links go (app_links.py). Prints
# every fault with the page, the link's words, where it goes and what is
# wrong; exits 1 on any.
#
#   run.py <out> <nginx.conf> <nginx> <verify> <gate env.json> <app web> <app src> <gate src>
import json
import os
import sys
import time
import urllib.parse

here = os.path.dirname(os.path.abspath(__file__))
sys.path[:0] = [here, os.path.join(here, "..", "site")]

from playwright.sync_api import sync_playwright  # noqa: E402

import a11y  # noqa: E402
import app_links  # noqa: E402
from crawl import Problem, chrome_args, crawl  # noqa: E402
from stack import Stack  # noqa: E402

out, conf, nginx, verify, env, app_web, app_src, gate_src = sys.argv[1:9]
tmp = os.environ.get("TMPDIR", "/tmp") + "/site-links"
os.makedirs(tmp, exist_ok=True)

started = time.monotonic()
problems, excused, opened = [], [], {}
with Stack(tmp, conf, nginx, verify, env) as s, sync_playwright() as p:
    home, front = s.url("home", "/"), s.url("", "/")

    def docs(url):
        return url.startswith(s.url("", "/docs"))

    # (who, how their browser finds the box, where it starts, what it
    # leaves to another view)
    views = [
        # the fleet's admin on the fleet's network, names as the world's DNS
        # answers them: every page, the docs, the front page
        ("david", "world", [home, front], lambda u: False),
        # the same with the network's own DNS: the pages whose answer
        # depends on the door; the docs are the same through either
        ("david", "network", [home], docs),
        # a stranger at the front page, not on the fleet's network; of the
        # docs, their front: a stranger's docs pages show a member's links
        # less the forge's, and the member's crawl opened every one
        (
            None,
            "outside",
            [front],
            lambda u: docs(u) and u.rstrip("/") != s.url("", "/docs"),
        ),
    ]
    # the accessibility pass (site/a11y.py): axe on every page the crawl
    # opens, once per person and page; serious and critical fail, but for
    # those a11y.KNOWN holds for main today
    audited = set()

    def accessible(who, way):
        def on_page(page, url):
            u = urllib.parse.urlsplit(url)
            path = u.path or "/"
            key = (who, u.hostname, path)
            if key in audited:
                return
            audited.add(key)
            for rule, impact, why, targets in a11y.audit(page):
                if (rule, path) in a11y.KNOWN:
                    excused.append((url, f"{impact} {rule}", a11y.KNOWN[(rule, path)]))
                    continue
                problems.append(
                    Problem(
                        who or "signed-out",
                        way,
                        url,
                        "",
                        "",
                        f"accessibility: {impact} {rule}: {why} at {targets}",
                    )
                )

        return on_page

    for who, way, starts, skip in views:
        t = time.monotonic()
        chrome = p.chromium.launch(args=chrome_args(s, way))
        try:
            found, e, seen = crawl(
                s,
                chrome,
                who,
                way,
                starts,
                skip=skip,
                on_page=accessible(who, way),
                app_takes=app_links.handoff_pages(app_src),
            )
        finally:
            chrome.close()
        problems += found
        excused += e
        opened[f"{who or 'signed-out'}/{way}"] = sorted(seen)
        slow = sorted(((v.get("took", 0), k) for k, v in seen.items()), reverse=True)[
            :3
        ]
        print("  slowest: " + ", ".join(f"{u} {t}s" for t, u in slow), flush=True)
        print(
            f"{who or 'signed out'}, the {way} way: {len(seen)} addresses in {time.monotonic() - t:.0f}s, {len(found)} faults",
            flush=True,
        )

app = app_links.check(app_web, app_src, gate_src)
print(f"accessibility: {len(audited)} pages audited", flush=True)
print(f"the app's pages: {app['links']} links, {len(app['faults'])} faults", flush=True)

# one entry per fault; a link wrong on every page (the bar's) once, with
# how many other pages show it
grouped = {}
for f in problems:
    grouped.setdefault(f.key(), []).append(f.page)
lines = []
for (who, way, link, target, what), pages in sorted(
    grouped.items(), key=lambda kv: (kv[0][4], kv[0][3], kv[0][1])
):
    more = f" (and {len(pages) - 1} other pages)" if len(pages) > 1 else ""
    where = f"link {link!r} -> {target}" if link or target else "the page itself"
    lines.append(f"[{who}, {way}] {pages[0]}{more}\n    {where}\n    {what}")
for f in app["faults"]:
    lines.append(
        f"[the app] {f['page']}\n    link {f['link']!r} -> {f['target']}\n    {f['what']}"
    )

os.makedirs(out, exist_ok=True)
json.dump(
    {
        "faults": [f.__dict__ for f in problems]
        + [{"app": True, **f} for f in app["faults"]],
        "excused": [{"page": a, "what": b, "why": c} for a, b, c in excused],
        "opened": opened,
        "seconds": round(time.monotonic() - started),
    },
    open(f"{out}/report.json", "w"),
    indent=1,
)
why = {}
for _, _, w in excused:
    why[w] = why.get(w, 0) + 1
for w, n in sorted(why.items()):
    print(f"excused {n}x: {w}")
print(f"{time.monotonic() - started:.0f}s in all")
if lines:
    print(f"\n{len(lines)} faults:\n")
    print("\n".join(lines))
    sys.exit(1)
print("every link opens a real page, in the same tab, and every page runs clean")
