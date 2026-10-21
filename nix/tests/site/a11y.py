# Accessibility (checks.site-a11y): axe-core on every page a member can
# open, as the site map lists them (/_dd/sitemap.json, the places Monty
# knows), and the pages of someone signed out or waiting, in Chromium
# against the gateway box's real nginx and gate (stack.py). Serious and
# critical findings fail; KNOWN holds the ones main has today, each with
# why, to fix, and fails when one of them is gone so the list only shrinks.
#
# audit(page) is the whole test of one page: the crawler (site-links) calls
# it on every page it reaches.
import json
import os
import sys
import time

from stack import Watch

AXE = os.environ.get("SITE_AXE", "")
IMPACTS = ("serious", "critical")

# (rule, page) -> why it is there today. A page is its path, without a
# query or a fragment. To be fixed; take a line out when it is.
KNOWN = {
    (
        "definition-list",
        "/_dd/settings",
    ): "Settings' Account and Sign-in sections put something other than <dt>/<dd> pairs (or <div>s of them) straight in their <dl> (servers/home/web/settings.js)",
    (
        "link-in-text-block",
        "/new",
    ): "Git's new-repository page has a link in its <small> fine print told apart by colour alone (servers/forge/web)",
}


_src = []


def _axe_source():
    if not _src:
        _src.append(open(AXE).read() + "\n;0")
    return _src[0]


def audit(page):
    """axe on the page as it is now: [(rule, impact, help, [targets])] of
    the serious and critical"""
    if not page.evaluate("() => !!window.axe"):
        # evaluated, not a <script>: the page's own policy (no inline
        # script) stays as it is
        page.evaluate(_axe_source())
    res = page.evaluate(
        """async () => {
          const r = await axe.run(document, {resultTypes: ['violations'],
            runOnly: {type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'best-practice']}});
          return r.violations.map(v => [v.id, v.impact, v.help, v.nodes.slice(0, 5).map(n => n.target.join(' '))]);
        }"""
    )
    return [v for v in res if v[1] in IMPACTS]


def places(s, ctx):
    """every page the site map has for a member, on its own name"""
    p = ctx.new_page()
    r = p.goto(s.url("home", "/_dd/sitemap.json"))
    m = json.loads(r.body())
    p.close()
    urls = set()

    def walk(v):
        if isinstance(v, dict):
            for k, x in v.items():
                if k in ("url", "href") and isinstance(x, str):
                    urls.add(x)
                else:
                    walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)

    walk(m)
    d = s.domain
    keep = set()
    for u in urls:
        if "{" in u:
            u = u.replace("{repo}", "david/commonty")
        if u.startswith("/"):
            u = s.url("home", u)
        if not u.startswith("https://"):
            continue
        host = u.split("/")[2]
        # pages the gate serves; a service behind it is a stand-in here, and
        # the app's pages are the app's (the journeys check runs them)
        if host == d or host.endswith("." + d):
            keep.add(u.split("#")[0])
    return sample(sorted(keep))


def sample(urls):
    """one page of each kind: the docs are one page drawn from many
    markdown files (two of each part are audited here; the crawler audits
    them all), and a repository's views one page each"""
    seen, out = {}, []
    for u in urls:
        host, _, path = u.split("://", 1)[1].partition("/")
        parts = path.split("/")
        if parts[0] == "docs" and len(parts) > 2:
            key, room = (host, "docs", parts[1]), 2
        elif host.startswith("git.") and len(parts) > 2:
            key, room = (host, "repo", parts[2].split("?")[0]), 1
        else:
            key, room = (host, path), 1
        if seen.get(key, 0) < room:
            seen[key] = seen.get(key, 0) + 1
            out.append(u)
    return out


def run(s, browser):
    t0 = time.monotonic()
    found = {}
    pages = []
    for who, urls in (
        (
            None,
            [
                s.url("@", "/"),
                s.url("@", "/docs"),
                s.url("home", "/_dd/login"),
                s.url("home", "/_dd/join"),
                s.url("home", "/_dd/download"),
            ],
        ),
        ("sam", [s.url("home", "/_dd/home")]),
        ("gwen", [s.url("home", "/_dd/home"), s.url("home", "/_dd/friends")]),
        ("mia", None),
    ):
        ctx = s.browser_context(browser, who=who)
        if urls is None:
            urls = places(s, ctx)
        p = ctx.new_page()
        w = Watch(p)
        for u in urls:
            try:
                r = p.goto(u, wait_until="load")
                if (
                    r is None
                    or r.status >= 400
                    or b"stand-in" in (r.body()[:9] if r else b"")
                ):
                    continue
                w.settled()
                key = (u.split("?")[0], who or "anon")
                pages.append(key)
                for rule, impact, why, targets in audit(p):
                    found[
                        (rule, u.split("://", 1)[1].split("/", 1)[1].split("?")[0])
                    ] = (impact, why, targets, who)
            finally:
                pass
        ctx.close()
    bad = []
    for (rule, path), (impact, why, targets, who) in sorted(found.items()):
        if ("/" + path) in [k[1] for k in KNOWN if k[0] == rule] or (
            rule,
            "/" + path,
        ) in KNOWN:
            continue
        bad.append(f"/{path} ({who or 'anon'}): {impact} {rule}: {why} at {targets}")
    for (rule, path), why in KNOWN.items():
        if (rule, path.lstrip("/")) not in found:
            bad.append(f"{path}: {rule} is fixed; take it off KNOWN ({why})")
    print(
        f"site-a11y: {len(pages)} pages in {time.monotonic() - t0:.1f}s, {len(KNOWN)} known"
    )
    for (rule, path), why in KNOWN.items():
        print(f"  known: {path} {rule}: {why}")
    for b in bad:
        print("FAIL", b)
    return not bad


if __name__ == "__main__":
    from playwright.sync_api import sync_playwright
    from stack import Stack

    out, conf, nginx, verify, env = sys.argv[1:6]
    with Stack(out + "/stack", conf, nginx, verify, env) as s, sync_playwright() as pw:
        browser = pw.chromium.launch(args=s.chrome_args())
        ok = run(s, browser)
        browser.close()
    sys.exit(0 if ok else 1)
