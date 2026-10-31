# What every name of the gateway box tells a browser (checks.site-headers),
# against its real nginx and gate (stack.py). The intent is written here
# from where it is decided, and any answer that drifts from it fails:
#
#   nix/modules/gate/headers.nix   HSTS, nosniff, Referrer-Policy on every
#                                  location; our content policy on what the
#                                  gate and the games manager serve; frame-
#                                  ancestors on the rest; Vaultwarden its own
#   servers/gate/src/session.rs    the session cookie: Secure, HttpOnly,
#                                  SameSite=Lax, 8 hours, for the domain
#   docs/dev/gate.md               "Headers, CSP and the cookie", "Public
#                                  names, the bare domain and the demo",
#                                  "Routes" (who may open what)
#   nix/modules/llm/llm.nix,       the only cross-origin reads: the model
#   nix/modules/forge/forgejo.nix  and the forge's API, by the site's own
#                                  names, with a session
#
# Four parts: headers on every name; the session cookie and the front page;
# every gate route asked signed out; CORS.
import re
import sys
import time

HSTS = "max-age=31536000; includeSubDomains"


def ours(d):
    """the content policy of our own pages (headers.nix `ours`), written out
    again here so a change to it is a change to this test too"""
    return "; ".join(
        [
            "default-src 'self'",
            "script-src 'self' 'wasm-unsafe-eval'",
            "style-src 'self' 'unsafe-inline'",
            "img-src 'self' data: blob: https:",
            f"media-src 'self' blob: https://*.{d}",
            f"connect-src 'self' https://{d} https://*.{d} wss://*.{d} https://api.themoviedb.org https://image.tmdb.org",
            "font-src 'self' data:",
            f"frame-src 'self' blob: https://*.{d}",
            "worker-src 'self' blob:",
            "object-src 'none'",
            "base-uri 'self'",
            f"form-action 'self' https://*.{d}",
            "frame-ancestors 'self'",
            "report-uri /_dd/csp",
        ]
    )


def directives(csp):
    out = {}
    for part in csp.split(";"):
        w = part.split()
        if w:
            out[w[0]] = set(w[1:])
    return out


# what each name is asked for: the root, a page file, the gate's own API,
# a page of the gate's, and an address nothing serves
PATHS = ["/", "/_dd/static/bar.css", "/_dd/me", "/_dd/home", "/no-such-page"]

# GET routes of the gate that answer someone signed out, and why (gate.md
# "Routes"). Every other route must refuse them: 401, 403, or the sign-in.
# A new route is refused unless it is added here.
PUBLIC = {
    "/_dd/login": "sign-in",
    "/_dd/join": "a browser account's page",
    "/_dd/enrol": "the enrol page (its POSTs need the token)",
    "/_dd/enrol/result": "dd collects a passkey with its token; none, nothing",
    "/_dd/demo": "a demo session for anyone",
    "/_dd/back": "back to our own page after a sign-in",
    "/_dd/guide-keep": "Monty's frame, no data",
    "/_dd/config": "rpId",
    "/_dd/static/{file}": "css, js, fonts",
    "/_dd/web/{file}": "the wasm bundle",
    "/_dd/add": "the add-device page (QR target)",
    "/_dd/add/wait": "the new device's side, with its code",
    "/_dd/send": "Passwords' Send page",
    "/_dd/download": "app downloads",
    "/_dd/docs/index.json": "the docs",
    "/_dd/docs/page": "the docs",
    "/_dd/docs/page/{*rest}": "the docs",
    "/_dd/docs/raw/{*path}": "the docs",
    "/_dd/git/public": "the forge's public door's page",
    "/_dd/photos": "hands a browser to the app",
    "/_dd/photos/forget": "Clear-Site-Data, then on",
    "/_dd/use-app": "hands a browser to the app",
    "/_dd/friend/{secret}": "a friend link's page",
    "/_dd/friend/{secret}/about": "anyone with the link",
    "/_dd/invite/{key}": "an invite, by its key",
    "/_dd/members": "the signed member list: ids only, in the open repository already",
    "/_dd/oidc/.well-known/openid-configuration": "OIDC discovery",
    "/_dd/oidc/jwks": "the OIDC signing key",
    "/_dd/controller-name": "the passkey naming page; it signs in itself",
    "/_dd/landing": "the front page",
    "/_dd/status": "the uptime page",
    "/_dd/status.json": "each service up, and its 90 days: names no box and no release (status.rs)",
    "/_dd/joke": "an easter egg page, no data",
    "/_dd/sso/start": "sign-in at the id Worker",
    "/_dd/sso/creds": "the passkey ids a name has, for the id Worker",
    "/_dd/boxes": "a redirect to Settings",
    "/_dd/backups": "a redirect to Settings",
    "/_dd/devices": "a redirect to Settings",
    "/_dd/network": "a redirect to Settings",
    "/_dd/storage": "a redirect to Settings",
    "/_dd/logout": "a page with the sign-out button",
    "/_dd/releases.json": "the newest release notes, from the public repository's releases (releases.rs)",
}
# answers that are neither, and carry nothing: the status, and why
OTHER = {
    "/_dd/git/events": (
        404,
        "Git's live changes are off without the forge's database (VERIFY_FORGE_EVENTS): 404 before anyone is asked",
    ),
    "/_dd/mail/pipe": (
        400,
        "a websocket, through the generic /_dd/ location, which sends no Upgrade on: the handshake fails before anyone is asked",
    ),
}
# the demo's own: someone signed out is sent to start the demo (gate.md
# "The demo"), which is a sign-in of its own
DEMO = {
    "/_dd/photos/demo",
    "/_dd/photos/demo/list",
    "/_dd/photos/demo/photo/{id}",
    "/_dd/photos/demo/thumb/{id}",
}
# routes that never reach the gate through nginx on a gated name
NOT_THROUGH_NGINX = {
    "/_dd/house/here",
    "/_dd/house/wifi/relay",
    "/verify",
    "/health",
    "/directory",
    "/identity",
    "/state",
    "/computer-lease",
}


def gate_routes(src):
    """the gate's GET routes under /_dd/, read from its router"""
    import glob

    text = "".join(open(f).read() for f in sorted(glob.glob(src + "/*.rs")))
    text = re.sub(r"\s+", " ", text)
    out = set()
    for path, how in re.findall(
        r'\.route\( ?"(/[^"]*)", ?((?:axum::routing::)?[a-z_]+)\(', text
    ):
        if path.startswith("/_dd/") and how.endswith("get"):
            out.add(path)
    return sorted(out)


def concrete(path):
    return (
        path.replace("{*rest}", "x")
        .replace("{*path}", "x")
        .replace("{file}", "nothing.js")
        .replace("{name}", "mia")
        .replace("{secret}", "x" * 32)
        .replace("{key}", "x" * 43)
        .replace("{label}", "x")
        .replace("{lib}", "0" * 32)
        .replace("{kind}", "progress")
        .replace("{id}", "1")
        # the query a route reads before it asks who is there
        + {
            "/_dd/chat/search": "?q=otters",
            "/_dd/mail/pipe": "?domain=x&host=x",
            "/_dd/add/status": "?code=x",
            "/_dd/git/events": "?repo=1",
            "/_dd/oidc/authorize": "?client_id=vaultwarden&redirect_uri=https%3A%2F%2Fvault.commonty.org%2Fidentity%2Fconnect%2Foidc-signin&response_type=code&scope=openid&state=x&nonce=x",
        }.get(path, "")
    )


def check_headers(s, fail):
    d = s.domain
    s.gate_names = (
        {d}
        | {
            f"{h}.{d}"
            for h in ("home", "files", "games", "git", "metrics", "llm", "photos")
        }
        | {f"demo-{h}.{d}" for h in ("files", "games", "git", "llm")}
    )
    seen = 0
    for host in s.hosts:
        sub = host[: -len(s.domain) - 1] if host != s.domain else "@"
        for outside in (False, True):
            for who in (None, "mia"):
                if outside and who:
                    continue
                for p in PATHS:
                    r = s.get(sub, p, who=who, outside=outside)
                    if r.status == 444:
                        continue
                    seen += 1
                    where = f"{'outside ' if outside else ''}{host}{p}{' as ' + who if who else ''} ({r.status})"
                    one(s, r, where, fail)
    return seen


def one(s, r, where, fail):
    # nginx's own answer for an address no location takes: none of the
    # location's headers (KNOWN below)
    if (
        r.status == 404
        and b"<center>nginx</center>" in r.body
        and not r.all("strict-transport-security")
    ):
        s.bare404.add(where.removeprefix("outside ").split("/")[0])
        return
    hsts = r.all("strict-transport-security")
    if hsts != [HSTS]:
        fail(f"{where}: Strict-Transport-Security {hsts}, want [{HSTS!r}]")
    # Vaultwarden sends its own set, and gets HSTS alone (headers.nix)
    if r.body.startswith(b"stand-in 127.0.0.1:8222"):
        return
    if r.all("x-content-type-options") != ["nosniff"]:
        fail(f"{where}: X-Content-Type-Options {r.all('x-content-type-options')}")
    if r.all("referrer-policy") != ["strict-origin-when-cross-origin"]:
        fail(f"{where}: Referrer-Policy {r.all('referrer-policy')}")
    csps = r.all("content-security-policy")
    if len(csps) != 1:
        fail(f"{where}: {len(csps)} Content-Security-Policy headers")
        return
    d = directives(csps[0])
    want = directives(ours(s.domain))
    if d.get("frame-ancestors") not in ({"'self'"}, {"'none'"}):
        fail(f"{where}: frame-ancestors {d.get('frame-ancestors')}")
    for k, v in d.items():
        # nothing beyond what our own pages allow, anywhere
        allowed = want.get(k, want["default-src"] if k.endswith("-src") else set())
        if k == "report-uri":
            continue
        extra = v - allowed - {"'none'"}
        if extra:
            fail(f"{where}: {k} allows {sorted(extra)} beyond ours")
    ctype = r.header("content-type") or ""
    # a page the gate made carries our policy exactly (the names whose pages
    # are the gate's; the forge's public door, code., has a stricter one of
    # its own, ddCsp; Photos' web apps their own files)
    host = where.removeprefix("outside ").split("/")[0]
    if (
        r.status == 200
        and "text/html" in ctype
        and not r.body.startswith(b"stand-in")
        and host in s.gate_names
    ):
        if csps[0] != ours(s.domain):
            fail(f"{where}: our page with policy {csps[0]!r}, want ours")


def flags(c):
    parts = [x.strip() for x in c.split(";")]
    name, _, value = parts[0].partition("=")
    attrs = {}
    for a in parts[1:]:
        k, _, v = a.partition("=")
        attrs[k.lower()] = v
    return name, value, attrs


def check_cookies(s, fail):
    d = s.domain
    # a session, as the gate issues one (the demo's: the same issue())
    r = s.get("home", "/_dd/demo")
    got = [c for c in r.all("set-cookie") if c.startswith("dd_session=")]
    if not got:
        fail(f"GET home/_dd/demo: no session cookie ({r.status})")
    for c in got:
        name, value, a = flags(c)
        for f in ("secure", "httponly"):
            if f not in a:
                fail(f"session cookie without {f}: {c.split(';')[0][:20]}...; {a}")
        if a.get("samesite", "").lower() != "lax":
            fail(f"session cookie SameSite={a.get('samesite')}, want Lax")
        if a.get("domain") != d or a.get("path") != "/":
            fail(
                f"session cookie Domain={a.get('domain')} Path={a.get('path')}, want {d} and /"
            )
        if not a.get("max-age", "").isdigit() or not 0 < int(a["max-age"]) <= 8 * 3600:
            fail(f"session cookie Max-Age={a.get('max-age')}, want at most 8 hours")
    # signing out clears it with the same flags
    r = s.get(
        "home",
        "/_dd/logout",
        who="mia",
        method="POST",
        headers={"origin": f"https://home.{d}"},
    )
    out = [c for c in r.all("set-cookie") if c.startswith("dd_session=")]
    if not out or any(
        "max-age=0" not in c.lower()
        or "secure" not in c.lower()
        or "httponly" not in c.lower()
        for c in out
    ):
        fail(
            f"POST home/_dd/logout: session not cleared with Secure; HttpOnly ({r.status} {out})"
        )
    # every other cookie the box sets: Secure and SameSite, never readable
    # by another site
    for host, path, who in (
        ("home", "/_dd/login", None),
        ("home", "/_dd/home", "mia"),
        ("files", "/_dd/files", "mia"),
    ):
        for c in s.get(host, path, who=who).all("set-cookie"):
            name, value, a = flags(c)
            if "secure" not in a or not a.get("samesite"):
                fail(f"{host}{path}: cookie {name} without Secure and SameSite")
    # the public front page and the docs never set a cookie, for anyone,
    # inside or out
    for path in ("/", "/docs", "/docs/guide/joining", "/download"):
        for who, outside in ((None, True), (None, False), ("mia", False)):
            r = s.get("@", path, who=who, outside=outside)
            if r.all("set-cookie"):
                fail(
                    f"{d}{path}{' as ' + who if who else ''}{' outside' if outside else ''}: sets {[c.split('=')[0] for c in r.all('set-cookie')]}"
                )
    # both of a session's names, anywhere in the header, stay at the gate and
    # the rest of the header goes on
    mark = len(s.seen)
    jar = f"a=1; __Host-dd=x~y.1.z; {s.cookie('mia')}; b=2"
    s.get("git", "/api/v1/version", who="mia", headers={"cookie": jar})
    rows = s.seen.since(mark)
    if not rows:
        fail("the jar with both session names reached no service")
    for row in rows:
        c = row[4].get("cookie", "")
        if "a=1" not in c or "b=2" not in c:
            fail(f"{row[0]} lost a service's own cookies: {c!r}")
    # a session under its __Host- name counts as a credential at the flood
    # limits, not as a stranger at the directory's one a second
    limited = [
        s.get(
            "home", "/_dd/directory/nobody", headers={"cookie": "__Host-dd=x~y.1.z"}
        ).status
        for _ in range(30)
    ]
    if 429 in limited or 503 in limited:
        fail(f"a __Host-dd browser was limited as a stranger: {limited}")
    # and a session never reaches a service behind the gate (strip-cookie.nix)
    for row in s.seen.since(0):
        c = row[4].get("cookie", "")
        if "dd_session=" in c or "__Host-dd=" in c:
            fail(f"the session cookie reached {row[0]} for {row[2]}{row[3]}")


def check_signed_out(s, src, fail):
    routes = gate_routes(src)
    if len(routes) < 60:
        fail(f"only {len(routes)} routes read from {src}: the router moved?")
    n = 0
    for route in routes:
        if route in PUBLIC or route in NOT_THROUGH_NGINX:
            continue
        for host in ("home", "git"):
            # a websocket's route is asked as one
            ws = (
                {
                    "connection": "Upgrade",
                    "upgrade": "websocket",
                    "sec-websocket-version": "13",
                    "sec-websocket-key": "dGhlIHNhbXBsZSBub25jZQ==",
                }
                if route == "/_dd/mail/pipe"
                else None
            )
            r = s.get(host, concrete(route), headers=ws)
            n += 1
            loc = r.header("location") or ""
            if r.status in (401, 403) or (
                r.status in (302, 303) and "/_dd/login" in loc
            ):
                continue
            if route in OTHER and r.status == OTHER[route][0] and len(r.body) < 200:
                continue
            if route in DEMO and r.status in (302, 303) and loc.endswith("/_dd/demo"):
                continue
            # a page that is the same everywhere is sent to home's: that one decides
            if r.status in (301, 302, 303) and loc.startswith(
                f"https://home.{s.domain}/_dd/"
            ):
                continue
            if r.status == 200 and b"stand-in" in r.body[:9]:
                fail(f"signed out, {host}{route}: reached a service ({r.status})")
            else:
                fail(
                    f"signed out, {host}{route}: {r.status} {loc} {r.body[:60]!r}, want 401, 403 or the sign-in"
                )
    # and every service name's own pages
    for host in ("files", "games", "git", "llm", "metrics"):
        for p in ("/", "/x/y"):
            r = s.get(host, p)
            n += 1
            loc = r.header("location") or ""
            ok = (
                r.status in (401, 403, 404)
                or "/_dd/login" in loc
                or (
                    r.status in (301, 302)
                    and loc.startswith(f"https://{host}.{s.domain}/_dd/")
                )
            )
            if not ok:
                fail(f"signed out, {host}{p}: {r.status} {loc} {r.body[:60]!r}")
    return n, len(routes)


def check_cors(s, fail):
    d = s.domain
    evil = "https://evil.example"
    site = f"https://home.{d}"
    asks = [
        ("llm", "/v1/models", "GET"),
        ("llm", "/v1/chat/completions", "POST"),
        ("git", "/api/v1/version", "GET"),
        ("git", "/api/v1/repos/david/commonty/issues", "POST"),
        ("metrics", "/api/v1/query?query=up", "GET"),
        ("home", "/_dd/me", "GET"),
        ("home", "/_dd/directory", "GET"),
        ("home", "/_dd/sso/creds?user=mia", "GET"),
        ("files", "/_dd/dav/" + "0" * 32 + "/", "GET"),
    ]
    for host, path, method in asks:
        for origin in (evil, site, "null"):
            for m in (method, "OPTIONS"):
                r = s.get(
                    host,
                    path,
                    who="david",
                    method=m,
                    headers={"origin": origin, "access-control-request-method": method},
                )
                acao = r.all("access-control-allow-origin")
                where = f"{m} {host}{path} from {origin}"
                if "*" in acao:
                    fail(f"{where}: Access-Control-Allow-Origin *")
                if origin != site and acao:
                    fail(f"{where}: lets {acao} read it")
    # the reads that are meant: the model and the forge's API, by our names
    for host, path, method, want in (
        ("llm", "/v1/models", "GET", True),
        ("git", "/api/v1/version", "GET", True),
        ("git", "/api/v1/repos/david/commonty/issues", "POST", False),
        ("home", "/_dd/me", "GET", False),
    ):
        r = s.get(host, path, who="david", method=method, headers={"origin": site})
        acao = r.all("access-control-allow-origin")
        if want and (
            acao != [site] or r.header("access-control-allow-credentials") != "true"
        ):
            fail(
                f"{method} {host}{path} from {site}: {acao}, want it let read with credentials"
            )
        if not want and acao:
            fail(
                f"{method} {host}{path} from {site}: {acao}, want no cross-origin read"
            )


# names whose unknown addresses nginx answers itself, bare (no `/` location):
# main today. A name that starts carrying the headers there, or a new one
# that stops, fails.
KNOWN_BARE_404 = {}


def check_names(s, fail):
    """each name is one server's on each port: a second is never answered
    (nginx takes the first, and only warns)"""
    conf = open(s.tmp + "/nginx.conf").read()
    seen = {}
    ports = []
    for line in conf.splitlines():
        w = line.split()
        if w[:2] == ["server", "{"]:
            ports = []
        elif w[:1] == ["listen"] and len(w) > 1:
            ports.append(w[1].rsplit(":", 1)[-1])
        elif w[:1] == ["server_name"]:
            for name in " ".join(w[1:]).rstrip(";").split():
                if name == "_":
                    continue
                for port in set(ports):
                    seen[(name, port)] = seen.get((name, port), 0) + 1
    for (name, port), n in sorted(seen.items()):
        if n > 1:
            fail(
                f"{name}: named by {n} servers on port {port}; nginx answers from the first"
            )


def run(s, src):
    t0 = time.monotonic()
    bad = []
    fail = bad.append
    s.bare404 = set()
    n = check_headers(s, fail)
    bare = {h.split(".")[0] if h != s.domain else "@" for h in s.bare404}
    for h in sorted(bare - set(KNOWN_BARE_404)):
        fail(f"{h}: nginx's own 404, without HSTS, nosniff or CSP")
    for h in sorted(set(KNOWN_BARE_404) - bare):
        fail(
            f"{h}: its unknown addresses carry the headers now; take it off KNOWN_BARE_404"
        )
    check_names(s, fail)
    check_cookies(s, fail)
    m, routes = check_signed_out(s, src, fail)
    check_cors(s, fail)
    print(
        f"site-headers: {n} answers on {len(s.hosts)} names, {routes} gate routes ({m} asks signed out), in {time.monotonic() - t0:.1f}s"
    )
    for b in dict.fromkeys(bad):
        print("FAIL", b)
    return not bad


if __name__ == "__main__":
    from stack import Stack

    out, conf, nginx, verify, env, src = sys.argv[1:7]
    with Stack(out + "/stack", conf, nginx, verify, env) as s:
        ok = run(s, src)
    sys.exit(0 if ok else 1)
