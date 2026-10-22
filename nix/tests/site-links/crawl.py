# Every link the site's pages show, followed in a browser after their code
# has run: the bar, the tiles, the menus, the docs, the footer, and the
# links scripts build. For each one on the site: it opens a real page (not
# an error, the public Send page or a "Nothing to open"), in the same tab,
# and is not a bare # where somewhere is meant. Off the site: well formed,
# never fetched. Every page opened must also run without a script error, a
# content-policy refusal or a failed request.
#
#     crawl(stack, chrome, who, way, starts, on_page=None) -> (faults, excused, opened)
#
# on_page(page, url) runs on every page once it has settled, but for an
# error page (the accessibility pass hooks in there).
#
# How the browser finds the box (`way`, chrome_args): the box has two
# doors, the tunnel's listener (the internet, through Cloudflare) and its
# own 443 on the fleet's network.
#   world    names as the world's DNS answers them: the public names (those
#            the box lets in from outside) at Cloudflare, the rest at the
#            box's address on the fleet's network, which this device is on
#   network  the fleet's network's own DNS (the app's network on): every
#            name at the box's address there
#   outside  not on the fleet's network: the public names at Cloudflare,
#            the rest unreachable
import re
import time
import urllib.parse
from dataclasses import dataclass

import stack as site

# deliberate exceptions, each with the reason it is one
APP_LINKS = [
    (r"^commonty://", "the app's own link: the system hands it to the installed app"),
    (r"^https?://[a-z0-9-]+\.localhost(:\d+)?/", "the app's server on this computer"),
    (r"^tauri://localhost/", "the app's own window"),
]
# names of the fleet that no box serves, each with where it is
OFF_BOX = {
    "mail": "the mail Worker, at Cloudflare",
    "id": "the sign-in Worker, at Cloudflare",
    "dns": "the DNS Worker, at Cloudflare",
    "control": "the control plane's Worker, at Cloudflare",
}
# requests the gate answers by asking a program on the box, stood in here
# by something that answers nonsense: its failure is the stand-in's
GATE_ASKS = [
    (r"^/_dd/adblock/", "the gate asks Pi-hole, stood in here"),
    (r"^/_dd/chat/search", "the gate asks SearXNG, stood in here"),
    (r"^/_dd/photos/demo/", "the gate asks Photos' server (museum), stood in here"),
]
# the libraries' files: the gate reads them from the bucket (garage), which
# is not here; what it says without them is not the page's doing
BUCKET = (r"^/_dd/(dav|library)/", "the gate reads the library's bucket, not run here")
# an open page (the docs, the forge's public name) asks whether anyone is
# signed in, and 401/403 is the answer it expects (shell.js me())
OPEN_ME = (
    r"^https://[^/]+/_dd/me$",
    ("401", "403"),
    "an open page asking whether anyone is signed in",
)
# the download page needs the app's signed manifest, which the gate reads
# from the forge's releases branch; the stack gives it none
NO_MANIFEST = (
    r"^/_dd/download$",
    "404",
    "no app manifest in the stack: the gate reads it from the forge",
)
PLACEHOLDERS = [
    r"^Nothing to open$",
    r"^Not found$",
    r"^Page not found$",
    r"^404\b",
    r"^502\b",
    r"^Bad Gateway$",
    r"^Something went wrong",
    r"^The Commonty app didn't answer$",
]
# a repository's own pages: the forge's, whose program is stood in here, so
# what they show is made up; asked whether they open, not crawled through
FORGE = re.compile(r"^/(?!_dd/)[^/]+/[^/]+(/|$)")
# files, not pages: asked for, not opened in the tab (a file's name in a
# Git address is a page, the forge's view of it)
ASSET = re.compile(r"^/(download|_dd/static|_dd/download)/")

WATCH = """
(() => {
  window.__dd = { opened: [], csp: [] };
  window.open = function (url, target) {
    window.__dd.opened.push({ url: String(url), target: String(target || ''), stack: (new Error().stack || '').split('\\n').slice(2, 4).join(' | ') });
    return null;
  };
  document.addEventListener('securitypolicyviolation', (e) => {
    window.__dd.csp.push(`${e.violatedDirective} refused ${e.blockedURI || 'inline'} (${e.sourceFile || ''}:${e.lineNumber || ''})`);
  });
})();
"""

ANCHORS = "a[href], area[href]"

LINKS = """
(sel) => {
  const base = document.querySelector('base[target]')?.getAttribute('target') || '';
  return [...document.querySelectorAll(sel)].map((a, i) => ({
    i,
    raw: a.getAttribute('href'),
    abs: a.href,
    text: (a.innerText || a.getAttribute('aria-label') || a.title || a.textContent || '').replace(/\\s+/g, ' ').trim().slice(0, 60),
    target: a.getAttribute('target') || base,
    download: a.hasAttribute('download'),
    role: a.getAttribute('role') || '',
    // shown now: a link in a tab not open yet is filled in when it opens
    shown: a.getClientRects().length > 0 && !a.closest('[hidden]'),
  }));
}
"""

SETTLED = """
() => ({
  title: document.title,
  h1: [...document.querySelectorAll('h1')].filter((h) => h.getClientRects().length > 0).map((h) => h.innerText.trim()),
  ids: [...document.querySelectorAll('[id], a[name]')].map((e) => e.id || e.getAttribute('name')),
  // the page that hands a browser to the member's app (use-app.js)
  handoff: !!document.querySelector('section#use-app'),
  opened: window.__dd ? window.__dd.opened : [],
  csp: window.__dd ? window.__dd.csp : [],
})
"""


@dataclass
class Problem:
    who: str
    way: str
    page: str
    link: str
    target: str
    what: str

    def key(self):
        return (self.who, self.way, self.link, self.target, self.what)


def public_names(s):
    """the names the box lets in from outside: gate/public.nix's map"""
    text = open(s.tmp + "/nginx.conf").read()
    return sorted(
        {n.replace("\\.", ".") for n in re.findall(r'"~\^0:([^$"]+)\$" 0;', text)}
    )


def chrome_args(s, way):
    tunnel, box = f"{site.HOST}:{site.TUNNEL}", f"{site.HOST}:{site.INSIDE}"
    rules = []
    if way in ("world", "outside"):
        rules += [f"MAP {n} {tunnel}" for n in public_names(s)]
    rest = box if way in ("world", "network") else "~NOTFOUND"
    rules += [f"MAP *.{s.domain} {rest}", f"MAP {s.domain} {rest}", "MAP * ~NOTFOUND"]
    return ["--mute-audio", "--host-resolver-rules=" + ", ".join(rules)]


def on_site(s, host):
    host = (host or "").lower()
    return host == s.domain or host.endswith("." + s.domain)


def well_formed(url):
    """an address off the site, by its shape alone"""
    try:
        u = urllib.parse.urlsplit(url)
    except ValueError as e:
        return str(e)
    if u.scheme in ("http", "https"):
        h = u.hostname or ""
        if not re.fullmatch(
            r"(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}|localhost|\d+\.\d+\.\d+\.\d+",
            h,
            re.I,
        ):
            return f"no usable host in {url!r}"
        if " " in url:
            return "a space in the address"
        return None
    if u.scheme == "mailto":
        return (
            None
            if re.fullmatch(r"[^@\s]+@[^@\s]+\.[^@\s]+", u.path)
            else f"not an address: {u.path!r}"
        )
    if u.scheme in ("tel", "commonty", "tauri"):
        return None
    return f"a {u.scheme or 'schemeless'} link"


def app_link(url):
    return next((why for pat, why in APP_LINKS if re.match(pat, url)), None)


def norm(url):
    u = urllib.parse.urlsplit(url)
    return urllib.parse.urlunsplit(
        (u.scheme, (u.hostname or "").lower(), u.path or "/", u.query, "")
    )


def excuse(s, code, addr, standins):
    """why a failed request is not the page's fault, or None"""
    u = urllib.parse.urlsplit(addr)
    sub = (u.hostname or "").split(".")[0]
    if on_site(s, u.hostname) and sub in OFF_BOX:
        return OFF_BOX[sub]
    if (u.hostname, u.path + (f"?{u.query}" if u.query else "")) in standins:
        return "answered by a stand-in"
    if re.search(BUCKET[0], u.path):
        return BUCKET[1]
    if code.startswith("5"):
        return next((w for p, w in GATE_ASKS if re.search(p, u.path)), None)
    if code in OPEN_ME[1] and re.match(OPEN_ME[0], addr):
        return OPEN_ME[2]
    if code == NO_MANIFEST[1] and re.search(NO_MANIFEST[0], u.path):
        return NO_MANIFEST[2]
    return None


def crawl(
    s,
    chrome,
    who,
    way,
    starts,
    on_page=None,
    skip=lambda url: False,
    limit=600,
    app_takes=None,
):
    """chrome: a Playwright browser launched with chrome_args(s, way);
    app_takes: the site's paths the app takes from a hand-off (handoff.rs)"""
    problems, excused = [], []
    ctx = s.browser_context(chrome, who=who)
    # what Cloudflare adds on its way through the tunnel. Not the visitor's
    # address: a public name's server takes CF-Connecting-IP on the box's
    # own door too, and would count a browser on the fleet's network as a
    # stranger.
    ctx.set_extra_http_headers({"x-forwarded-proto": "https"})
    ctx.add_init_script(WATCH)
    page = ctx.new_page()
    watch = site.Watch(page)
    cdp = ctx.new_cdp_session(page)
    seen = {}  # url -> what opening it showed
    queue = [norm(u) for u in starts]
    links_to = {u: [("(where this view starts)", "(start)")] for u in queue}

    def fail(at, link, target, what):
        problems.append(Problem(who or "signed-out", way, at, link, target, what))

    def has_click(i):
        o = cdp.send(
            "Runtime.evaluate",
            {"expression": f"document.querySelectorAll({ANCHORS!r})[{i}]"},
        )
        oid = o.get("result", {}).get("objectId")
        if not oid:
            return False
        ls = cdp.send("DOMDebugger.getEventListeners", {"objectId": oid, "depth": 0})[
            "listeners"
        ]
        return any(
            x["type"] in ("click", "mousedown", "pointerdown", "keydown") for x in ls
        )

    while queue and len(seen) < limit:
        url = queue.pop(0)
        if url in seen:
            continue
        log = {"console": [], "errors": [], "failed": []}

        def on_console(m, log=log):
            if m.type == "error":
                log["console"].append((m.text, (m.location or {}).get("url", "")))

        def on_error(e, log=log):
            log["errors"].append(str(e).splitlines()[0][:200])

        def on_response(r, log=log):
            if r.status >= 400:
                log["failed"].append((str(r.status), r.request.method, r.url))

        def on_failed(r, log=log):
            err = r.failure or "failed"
            # the page called it off (an AbortController, or leaving), not
            # the request failing
            if "ERR_ABORTED" in err:
                return
            log["failed"].append((err.replace(" ", "_"), r.method, r.url))

        page.on("console", on_console)
        page.on("pageerror", on_error)
        page.on("response", on_response)
        page.on("requestfailed", on_failed)
        before = len(s.seen)
        began = time.monotonic()
        state = {"status": 0, "error": None, "final": url}
        try:
            resp = page.goto(url, wait_until="load", timeout=30000)
            state["status"] = resp.status if resp else 0
            plain = resp is not None and resp.headers.get(
                "content-type", ""
            ).startswith("text/plain")
            state["standin"] = plain and resp.text().startswith("stand-in ")
        except Exception as e:
            state["error"] = str(e).split("\n")[0]
        if not state["error"]:
            try:
                # settled: the bar drawn, and everything the page asked for
                # answered (a stream that stays open aside)
                watch.settled(timeout=60)
            except Exception as e:
                state["error"] = "never settled: " + str(e).splitlines()[0][:200]
            state.update(page.evaluate(SETTLED))
            state["final"] = page.url
        # what reached a stand-in while this page was open: the page's own
        # program is not here, and what the page made of its answers is not
        # the page's to answer for
        standins = {(h.split(":")[0], p) for _, _, h, p, _ in s.seen.since(before)}
        state["stood"] = bool(standins)
        for event, fn in (
            ("console", on_console),
            ("pageerror", on_error),
            ("response", on_response),
            ("requestfailed", on_failed),
        ):
            page.remove_listener(event, fn)
        state["took"] = round(time.monotonic() - began, 2)
        seen[url] = state
        if norm(state["final"]) != url:
            seen.setdefault(norm(state["final"]), state)
        if state["error"] or state.get("standin"):
            continue
        at = state["final"]
        for text, where in log["console"]:
            # the browser's line for a failed request: the request is judged
            # below, with its address
            if text.startswith("Failed to load resource"):
                continue
            if standins:
                excused.append(
                    (
                        at,
                        text.splitlines()[0][:200],
                        "the page's own program is stood in here",
                    )
                )
                continue
            fail(at, "", "", f"console error: {text.splitlines()[0][:200]}")
        for e in log["errors"]:
            if standins:
                excused.append((at, e, "the page's own program is stood in here"))
                continue
            fail(at, "", "", f"script error: {e}")
        for c in state.get("csp", []):
            fail(at, "", "", f"content policy: {c}")
        for code, method, addr in log["failed"]:
            why = excuse(s, code, addr, standins)
            if why:
                excused.append((at, f"{code} {method} {addr}", why))
                continue
            fail(at, "", addr, f"request failed: {code} {method} {addr}")
        for o in state.get("opened", []):
            u = urllib.parse.urlsplit(urllib.parse.urljoin(at, o["url"]))
            if u.scheme in ("http", "https") and not on_site(s, u.hostname):
                continue
            fail(
                at,
                "(script)",
                o["url"],
                f"opens a new tab: window.open from {o['stack']}",
            )
        # an error page is judged by its status, where its links are
        if on_page and state["status"] < 400:
            on_page(page, at)
        if not on_site(s, urllib.parse.urlsplit(at).hostname):
            continue
        # a page drawn from a stand-in's answers links to made-up places;
        # its bar is the same as every other page's, checked there
        if state["stood"]:
            continue
        ids = set(state.get("ids", []))
        for a in page.evaluate(LINKS, ANCHORS):
            raw, abs_, text = a["raw"], a["abs"], a["text"] or "(no text)"
            if raw.strip() in ("", "#"):
                if not a["shown"] or a["role"] == "button" or has_click(a["i"]):
                    continue
                fail(
                    at,
                    text,
                    raw,
                    "goes nowhere: href is empty or # with nothing behind it",
                )
                continue
            if raw.startswith("#"):
                frag = urllib.parse.unquote(raw[1:])
                if "/" not in frag and frag not in ids and not has_click(a["i"]):
                    fail(at, text, raw, f"no #{frag} on the page")
                continue
            if abs_.lower().startswith("javascript:"):
                fail(at, text, raw, "a javascript: link")
                continue
            if app_link(abs_):
                continue
            u = urllib.parse.urlsplit(abs_)
            if u.scheme not in ("http", "https") or not on_site(s, u.hostname):
                bad = well_formed(abs_)
                if bad:
                    fail(at, text, abs_, f"malformed: {bad}")
                # a link off the site may open beside it
                continue
            if a["target"] and a["target"] not in ("_self", "_top", "_parent"):
                fail(at, text, abs_, f"opens a new tab (target={a['target']})")
            if u.hostname.split(".")[0] in OFF_BOX:
                continue
            target = norm(abs_)
            links_to.setdefault(target, []).append((at, text))
            if a["download"] or ASSET.search(u.path):
                continue
            if re.match(r"^(demo-)?(git|code)\.", u.hostname) and FORGE.match(u.path):
                continue
            if target not in seen and target not in queue and not skip(target):
                queue.append(target)

    # every link, judged by what opening it showed. The forge's pages are
    # asked one per route: the docs link hundreds of files, and the box
    # answers each of a repository's blob addresses alike
    by_route = {}
    for target, sources in links_to.items():
        state = seen.get(target)
        if state is None:
            if skip(target):
                continue
            route = forge_route(target)
            if route and route in by_route:
                state = by_route[route]
            else:
                state = seen[target] = resolve(s, who, way, target)
                if route:
                    by_route[route] = state
        what = judge(s, state, target, who is not None, app_takes)
        if what:
            for at, text in sources:
                fail(at, text, target, what)
    page.close()
    ctx.close()
    if len(seen) >= limit:
        problems.append(
            Problem(
                who or "signed-out",
                way,
                "",
                "",
                "",
                f"stopped at {limit} pages: raise the limit or skip more",
            )
        )
    return problems, excused, seen


def forge_route(url):
    """a forge address by its route: host, owner, repository and view"""
    u = urllib.parse.urlsplit(url)
    if not re.match(r"^(demo-)?(git|code)\.", u.hostname or "") or not FORGE.match(
        u.path
    ):
        return None
    return (u.hostname, *u.path.strip("/").split("/")[:3])


def resolve(s, who, way, url):
    """A file, a download, or a page whose program is stood in: asked
    through the door this way of finding the box would knock on, redirects
    followed on the site, not rendered. Not with the browser's own request
    client: it resolves names itself, past Chrome's host rules, so it would
    ask the real site."""
    public = set(public_names(s))
    for _ in range(10):
        u = urllib.parse.urlsplit(url)
        if not on_site(s, u.hostname):
            return {"status": 200, "error": None, "final": url}
        outside = u.hostname in public and way != "network"
        if way == "outside" and not outside:
            return {"status": 0, "error": "net::ERR_NAME_NOT_RESOLVED", "final": url}
        host = "" if u.hostname == s.domain else u.hostname[: -len(s.domain) - 1]
        r = s.get(
            host, u.path + (f"?{u.query}" if u.query else ""), who=who, outside=outside
        )
        if r.status == 444:
            return {"status": 0, "error": "net::ERR_EMPTY_RESPONSE", "final": url}
        if r.status in (301, 302, 303, 307, 308) and r.header("location"):
            url = urllib.parse.urljoin(url, r.header("location"))
            continue
        return {
            "status": r.status,
            "error": None,
            "final": url,
            "standin": r.body[:9] == b"stand-in ",
        }
    return {"status": 0, "error": "too many redirects", "final": url}


def judge(s, state, target, signed_in, app_takes=None):
    if state.get("standin"):
        return None
    if state.get("error"):
        e = state["error"]
        if "ERR_EMPTY_RESPONSE" in e or "ERR_CONNECTION_CLOSED" in e:
            return "the door is shut from here: nginx closed the connection (444)"
        if "ERR_NAME_NOT_RESOLVED" in e:
            return "unreachable from here: the name is only on the fleet's network"
        if e.startswith("never settled") and (
            state.get("stood") or any(re.search(p.lstrip("^"), e) for p, _ in GATE_ASKS)
        ):
            # still waiting on a program stood in here, or one the gate asks
            return None
        return f"does not open: {e}"
    final = state["final"]
    fu = urllib.parse.urlsplit(final)
    if state["status"] >= 400:
        if state["status"] == 404 and re.search(NO_MANIFEST[0], fu.path):
            return None
        return f"answers {state['status']}" + (
            f" at {final}" if norm(final) != target else ""
        )
    if state.get("title", "").startswith("A Send") and "/send" not in target:
        return f"lands on the public Send page ({final}) showing {' / '.join(state.get('h1') or ['?'])!r}"
    for h in state.get("h1") or []:
        if any(re.search(p, h, re.I) for p in PLACEHOLDERS):
            return f"lands on a placeholder: {h!r} at {final}"
    # handed to the app: the app must take the page (Calendar and Contacts
    # went nowhere while the app's list lacked them)
    host = fu.hostname or ""
    if (
        state.get("handoff")
        and app_takes is not None
        and not re.match(r"^(vault|photos)\.", host)
    ):
        path = "/_dd/chat" if host.startswith("llm.") else fu.path
        if path not in app_takes and not path.startswith("/_dd/photos"):
            return f"handed to the app, which takes no {path} (app/src/handoff.rs SITE_PAGES)"
    if signed_in and (
        fu.path.startswith("/_dd/login") or (fu.hostname or "").startswith("id.")
    ):
        return f"asks a signed-in member to sign in again ({final})"
    if not on_site(s, fu.hostname) and not app_link(final):
        return f"leaves the site for {final}"
    return None
