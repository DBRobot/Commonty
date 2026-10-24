#!/usr/bin/env python3
# A ci run killed from outside - the forge or a runner restarting under it,
# as a release that changes either one does - is run again, once. Its jobs
# end "failed" with "context canceled" in their logs, and stop-run then
# cancels the rest, so the run looks red though nothing in it failed.
#
# A run is retried when every job that failed was killed mid-step, no newer
# run exists for its pull request (a newer push supersedes it), or for
# main (its full run), and it has not been retried here before. Runs every few minutes (a timer), as the
# admin through forgejo's socket, like ci-cancel.py: rerunning a run is a
# web route the api does not have.
# Environment: FORGE (the unix socket), ADMIN, REPO (owner/name), STATE (a
# directory this keeps the retried runs in).
import http.client
import json
import os
import re
import socket
import sys
import time
from datetime import datetime

forge = os.environ["FORGE"]
admin = os.environ["ADMIN"]
repo = os.environ["REPO"]
state = os.path.join(os.environ["STATE"], "retried.json")
# how far back a killed run is still worth running again
WINDOW = 3 * 3600
KILLED = "context canceled"


class UnixConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout):
        super().__init__("localhost", timeout=timeout)
        self.unix = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.unix)


def ask(method, path):
    """forgejo, as the admin, over its socket: (status, body bytes)"""
    c = UnixConnection(forge, 30)
    try:
        c.request(
            method,
            path,
            body=b"" if method == "POST" else None,
            headers={"X-WEBAUTH-USER": admin},
        )
        r = c.getresponse()
        return r.status, r.read()
    finally:
        c.close()


def mine(run):
    """a pull request's run, or main's full run; never a push to another
    branch (a member's own CI, say), which is not this one's to rerun"""
    return run.get("event") == "pull_request" or (
        run.get("event") == "push" and run.get("prettyref") == "main"
    )


def killed_not_failed(run):
    """every job of `run` that failed was killed, and at least one was"""
    status, body = ask(
        "GET",
        "/api/v1/repos/%s/commits/%s/statuses?limit=100" % (repo, run["commit_sha"]),
    )
    if status != 200:
        return False
    mine = re.compile(r"/actions/runs/%d/jobs/(\d+)$" % run["index_in_repo"])
    jobs = {}
    for s in json.loads(body):  # newest first: the first per job is its last word
        m = mine.search(s.get("target_url") or "")
        if m and m.group(1) not in jobs:
            jobs[m.group(1)] = s
    failed = [
        j
        for j, s in jobs.items()
        if s.get("status") == "failure"
        and not (s.get("description") or "").startswith("Has been cancelled")
    ]
    if not failed:
        return False
    for j in failed:
        status, log = ask(
            "GET",
            "/%s/actions/runs/%d/jobs/%s/attempt/1/logs"
            % (repo, run["index_in_repo"], j),
        )
        if status != 200 or KILLED.encode() not in log[-4000:]:
            return False
    return True


def main():
    try:
        retried = set(json.load(open(state)))
    except (OSError, ValueError):
        retried = set()
    # the forge down or restarting (a switch, say) is not a failure of
    # this: there is nothing to rerun until it answers, and the next run
    # looks again
    try:
        status, body = ask("GET", "/api/v1/repos/%s/actions/runs?limit=50" % repo)
    except (OSError, http.client.HTTPException) as e:
        print("the forge is not answering (%s); next time" % e)
        return 0
    if status != 200:
        print("the forge did not list its runs: http %s; next time" % status)
        return 0
    runs = json.loads(body).get("workflow_runs") or []
    now = time.time()
    newest = {}  # the latest run per pull request
    for r in runs:
        ref = r.get("prettyref")
        if mine(r) and ref and r["index_in_repo"] > newest.get(ref, 0):
            newest[ref] = r["index_in_repo"]
    for r in runs:
        if not mine(r) or r.get("status") not in (
            "failure",
            "cancelled",
        ):
            continue
        if r["index_in_repo"] != newest.get(r.get("prettyref")) or r["id"] in retried:
            continue
        stopped = r.get("stopped") or ""
        try:
            age = now - datetime.fromisoformat(stopped).timestamp()
        except ValueError:
            continue
        if age > WINDOW:
            continue
        try:
            if not killed_not_failed(r):
                continue
        except (OSError, http.client.HTTPException, ValueError) as e:
            print(
                "run #%d: not read (%s); next time" % (r["index_in_repo"], e),
                flush=True,
            )
            continue
        code, said = ask(
            "POST", "/%s/actions/runs/%d/rerun" % (repo, r["index_in_repo"])
        )
        print(
            "run #%d (%s): killed from outside, run again -> %s %s"
            % (r["index_in_repo"], r.get("prettyref"), code, said[:200]),
            flush=True,
        )
        # only a rerun that happened counts: the forge refuses one while any
        # job of the run is still going (run 786's was, and was never tried
        # again), so a refusal is "next time"
        if code == 200:
            retried.add(r["id"])
    os.makedirs(os.path.dirname(state), exist_ok=True)
    with open(state, "w") as f:
        json.dump(sorted(retried)[-500:], f)
    return 0


if __name__ == "__main__":
    sys.exit(main())
