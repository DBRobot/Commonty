"""Vaultwarden's /css/vaultwarden.css, written at build (modules/vault/vaultwarden.nix).

The app serves the web vault from its own release and asks no box for code,
so the stylesheet the server renders on each request is made here instead:
upstream's template, our part in it, the server's settings and the web
vault's version, the same helpers its handlebars has. Prints the SCSS; grass,
the compiler Vaultwarden itself uses, makes the CSS.

usage: vaultwarden-css.py TEMPLATE USER_TEMPLATE SETTINGS_JSON WEB_VERSION
"""

import json
import re
import sys

from pybars import Compiler


def version(s):
    return tuple(int(x) for x in s.split("+")[0].split("."))


def main():
    template, user, settings, web = sys.argv[1:5]
    with open(template) as f:
        src = f.read()
    with open(user) as f:
        mine = f.read()
    partial = "{{> scss/user.vaultwarden.scss }}"
    if partial not in src:
        sys.exit("upstream's template no longer includes the user stylesheet as before")
    src = src.replace(partial, mine)
    have = version(web)

    # semver requirements as upstream writes them: one operator, one version
    def webver(_this, req):
        m = re.fullmatch(r"(>=|<=|>|<|=)?\s*([0-9.]+)", req)
        if not m:
            sys.exit(f"webver {req!r}: not a requirement this script reads")
        want = version(m.group(2))
        return {
            ">=": have >= want,
            "<=": have <= want,
            ">": have > want,
            "<": have < want,
        }.get(m.group(1) or "=", have == want)

    helpers = {
        "webver": webver,
        "not": lambda _this, a: not a,
        "or": lambda _this, *a: any(a),
    }
    with open(settings) as f:
        ctx = json.load(f)
    sys.stdout.write(str(Compiler().compile(src)(ctx, helpers=helpers)))


main()
