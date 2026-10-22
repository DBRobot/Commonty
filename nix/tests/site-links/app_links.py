# The app's own pages (app/web, served by the app at localhost), read
# rather than run: every link in them goes to a route the app serves
# (app/src: the site's pages it carries, its /_dd/app/ routes, its files)
# or one the site serves (the gate's routes, the docs), is well formed when
# it leaves, and is never a bare # that nothing picks up.
import glob
import html.parser
import os
import re

from crawl import app_link, well_formed

# links that open a new tab on purpose, by page and id, each with why
LINK_NEW_TAB_OK = {
    (
        "mail.html",
        "confirm",
    ): "the mail Worker's passkey page opens beside Mail, which waits for it to finish",
}
# scripts that open a new tab on purpose, each with why
SCRIPT_NEW_TAB_OK = [
    ("chat.js", r"_blank", "a search result is someone else's site"),
    ("mail.js", r"_blank", "a link inside a message leads off the site"),
]


class Links(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.found = []
        self.ids = set()
        self.text = None

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if a.get("id"):
            self.ids.add(a["id"])
        if tag in ("a", "area") and "href" in a:
            self.found.append(
                {
                    "href": a["href"] or "",
                    "id": a.get("id", ""),
                    "data": [k[5:] for k in a if k.startswith("data-")],
                    "target": a.get("target", ""),
                    "line": self.getpos()[0],
                    "text": "",
                }
            )
            self.text = self.found[-1]

    def handle_data(self, data):
        if self.text is not None:
            self.text["text"] += data

    def handle_endtag(self, tag):
        if tag in ("a", "area"):
            self.text = None


def routes(app_web, app_src, gate_src):
    app, site = set(), {"/", "/docs"}
    for f in glob.glob(f"{app_src}/*.rs"):
        s = open(f).read()
        # the site's pages the app carries (site.rs page_for), its own routes
        app |= set(re.findall(r'"(/_dd/[A-Za-z0-9_./-]*)"', s))
        app |= set(re.findall(r'"(/[a-z][A-Za-z0-9_./-]*)"\s*(?:\||=>)', s))
    for f in glob.glob(f"{app_web}/_dd/static/*"):
        app.add("/_dd/static/" + os.path.basename(f))
    for f in glob.glob(f"{app_web}/*"):
        app.add("/" + os.path.basename(f))
    for f in glob.glob(f"{gate_src}/*.rs"):
        site |= set(re.findall(r'\.route\(\s*"([^"]+)"', open(f).read()))
    return app, site


def served(path, app, site):
    if path in app or path in site:
        return True
    if path.startswith("/docs/"):
        return True
    # a route with a parameter: /_dd/x/{name}
    for r in site | app:
        if "{" in r or ":" in r:
            pat = "^" + re.sub(r"\\\{[^}]*\\\}|:[a-z_]+", "[^/]+", re.escape(r)) + "$"
            if re.match(pat, path):
                return True
        if r.endswith("/") and path.startswith(r):
            return True
    return False


def check(app_web, app_src, gate_src):
    app, site = routes(app_web, app_src, gate_src)
    scripts = "".join(
        open(f).read() for f in glob.glob(f"{app_web}/**/*.js", recursive=True)
    )
    faults, n = [], 0
    pages = glob.glob(f"{app_web}/*.html") + glob.glob(f"{app_web}/_dd/pages/*.html")
    for f in sorted(pages):
        p = Links()
        p.feed(open(f).read())
        name = os.path.relpath(f, os.path.dirname(app_web))
        for a in p.found:
            n += 1
            href, text = (
                a["href"].strip(),
                re.sub(r"\s+", " ", a["text"]).strip() or "(no text)",
            )
            at = f"{name}:{a['line']}"

            def fault(what):
                faults.append({"page": at, "link": text, "target": href, "what": what})

            if (
                a["target"] == "_blank"
                and (os.path.basename(f), a["id"]) not in LINK_NEW_TAB_OK
            ):
                fault("opens a new tab (target=_blank)")
            if href in ("", "#"):
                # a script fills it in or takes the click: it names it, by
                # its id or by a data- attribute it selects
                named = a["id"] and re.search(
                    rf"['\"`#]{re.escape(a['id'])}['\"`]", scripts
                )
                picked = any(
                    re.search(rf"\[data-{re.escape(d)}[\]=]", scripts)
                    for d in a["data"]
                )
                if not (named or picked):
                    fault("goes nowhere: href is empty or # and no script names it")
                continue
            if href.startswith("#"):
                if href[1:] not in p.ids and not re.search(
                    rf"['\"`]{re.escape(href[1:])}['\"`]", scripts
                ):
                    fault(f"no {href} on the page")
                continue
            if app_link(href):
                continue
            if re.match(r"^[a-z][a-z0-9+.-]*:", href, re.I):
                bad = well_formed(href)
                if bad:
                    fault(f"malformed: {bad}")
                continue
            path = href.split("#")[0].split("?")[0]
            if not path.startswith("/"):
                path = "/" + os.path.normpath(
                    os.path.join(os.path.dirname(os.path.relpath(f, app_web)), path)
                ).lstrip("./")
            if not served(path, app, site):
                fault("neither the app nor the site serves this")
    # the links the app's scripts build out of a fixed address
    for f in sorted(glob.glob(f"{app_web}/**/*.js", recursive=True)):
        for i, line in enumerate(open(f), 1):
            for m in re.finditer(r"href\s*[=:]\s*['\"`](/[^'\"`$#?]*)", line):
                n += 1
                if not served(m.group(1), app, site):
                    faults.append(
                        {
                            "page": f"{os.path.relpath(f, os.path.dirname(app_web))}:{i}",
                            "link": "(built by the script)",
                            "target": m.group(1),
                            "what": "neither the app nor the site serves this",
                        }
                    )
            for m in re.finditer(r"window\.open\(|target\s*[=:]\s*['\"`]_blank", line):
                if any(
                    os.path.basename(f) == n and re.search(p, line)
                    for n, p, _ in SCRIPT_NEW_TAB_OK
                ):
                    continue
                faults.append(
                    {
                        "page": f"{os.path.relpath(f, os.path.dirname(app_web))}:{i}",
                        "link": "(built by the script)",
                        "target": line.strip()[:80],
                        "what": "opens a new tab",
                    }
                )
    return {"links": n, "faults": faults}


def handoff_pages(app_src):
    """the site's pages the app takes from a browser's hand-off"""
    src = open(f"{app_src}/handoff.rs").read()
    block = re.search(r"pub const SITE_PAGES: &\[&str\] = &\[(.*?)\];", src, re.S)
    return set(re.findall(r'"([^"]+)"', block.group(1))) if block else set()
