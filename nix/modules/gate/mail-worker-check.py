#!/usr/bin/env python3
# Is a Worker running at Cloudflare the one this release carries? (the mail
# Worker, or with WORKER=dns the DNS Worker)
# `dd release publish` uploads it from the laptop; the boxes vouched for its
# build (it is in this box's closure, at BUNDLE). Here every module of the
# deployed script is fetched with a token that can read scripts and nothing
# else - not their secrets - and compared byte for byte.
# Environment: CF_WORKERS_READ_TOKEN, ACCOUNT, SCRIPT, BUNDLE, FACTS; WORKER
# (default mail); DOWN_OK=1: Cloudflare out of reach is no failure of the
# unit (its fact still says 0).
import email.parser
import email.policy
import os
import sys
import time
import urllib.request

API = os.environ.get("CF_API", "https://api.cloudflare.com/client/v4")


def deployed(account, script, token):
    req = urllib.request.Request(
        "%s/accounts/%s/workers/scripts/%s" % (API, account, script),
        headers={"Authorization": "Bearer " + token},
    )
    with urllib.request.urlopen(req, timeout=60) as r:
        return parts(r.headers.get("content-type", ""), r.read())


def parts(content_type, body):
    """the modules of a script as Cloudflare hands it back: multipart, one
    part per module, named by its file"""
    msg = email.parser.BytesParser(policy=email.policy.HTTP).parsebytes(
        b"Content-Type: " + content_type.encode() + b"\r\n\r\n" + body
    )
    out = {}
    for p in msg.iter_parts():
        name = p.get_param("name", header="content-disposition")
        # the upload's own description of the script, if handed back
        if name and name != "metadata":
            out[name] = p.get_payload(decode=True) or b""
    return out


def compare(bundle, got):
    want = {n: open(os.path.join(bundle, n), "rb").read() for n in os.listdir(bundle)}
    wrong = sorted(n for n in want if got.get(n) != want[n])
    extra = sorted(n for n in got if n not in want)
    return wrong, extra


def main():
    facts = os.environ.get("FACTS")
    what = os.environ.get("WORKER", "mail")
    ok = 0
    down = False
    try:
        got = deployed(
            os.environ["ACCOUNT"],
            os.environ["SCRIPT"],
            os.environ["CF_WORKERS_READ_TOKEN"],
        )
        wrong, extra = compare(os.environ["BUNDLE"], got)
        for n in wrong:
            print("differs from this release: %s" % n, file=sys.stderr)
        for n in extra:
            print("not in this release: %s" % n, file=sys.stderr)
        ok = int(not wrong and not extra)
        if ok:
            print(
                "the %s Worker at Cloudflare is this release's, all %d files"
                % (what, len(got))
            )
    except Exception as e:  # noqa: BLE001 - any failure is "not shown to match"
        print("could not compare: %s" % e, file=sys.stderr)
        down = True
    if facts:
        tmp = os.path.join(facts, "%s-worker.prom.tmp" % what)
        with open(tmp, "w") as f:
            f.write(
                "# HELP dd_%s_worker_matches 1 when the deployed %s Worker is this release's\n"
                % (what, what)
            )
            f.write("# TYPE dd_%s_worker_matches gauge\n" % what)
            f.write("dd_%s_worker_matches %d\n" % (what, ok))
            f.write("dd_%s_worker_checked_seconds %d\n" % (what, int(time.time())))
        os.replace(tmp, os.path.join(facts, "%s-worker.prom" % what))
    sys.exit(0 if ok or (down and os.environ.get("DOWN_OK") == "1") else 1)


if __name__ == "__main__":
    main()
