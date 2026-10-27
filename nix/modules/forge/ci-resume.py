#!/usr/bin/env python3
# A CI job killed from outside - the forge or a runner restarting or moving
# under it, as a release or a failover does - is run again by itself, and
# the jobs that need it (ci_ok among them) run again after it; the rest of
# the run keeps what it already passed.
#
# Two ways a job dies of a restart, both seen on the fleet:
#   - its runner stopped under it: the job ends "failed" and its log's last
#     line is "context canceled" (runs 771, 786);
#   - the forge went away: the runner cannot report, its log stops in the
#     middle of a step with no end line ("🏁 ..."), and the forge, back up,
#     marks the task failed once it has heard nothing for its zombie
#     timeout, 10 minutes (runs 873, 880, 881, 884, and 900 across the
#     forge's move of 2026-10-25).
# A job that ended itself - its log's end line, then no "context canceled"
# - failed for real and is never run again; nor one stopped for running
# past the forge's three hours (a hang), nor one that failed in its first
# moments with its runner still there (a full disk, run 850).
#
# The forge's own per-job rerun (POST .../runs/<run>/jobs/<job>/rerun)
# runs that job again and every job that needs it, ci_ok too, at once
# even while the rest of the run is still going; ci_ok reads its needs'
# latest results, so a run whose killed jobs pass on the rerun goes green.
#
# Only a pull request's run or main's full run, of the CI workflow, the
# newest for its ref (a newer push supersedes it), within two days, and no
# job past its third attempt. The attempt count is the forge's own record,
# so nothing is kept on this box: a forge that moves takes it along, and
# whichever box the forge runs on (placement, its needs' units) carries on.
# A run with a job that failed for real, or one that was cancelled, is left
# as it is.
#
# Runs every few minutes (a timer), as the boxes' CI identity through
# forgejo's socket, like ci-cancel.py: rerunning is a web route the api
# does not have.
# Environment: FORGE (the unix socket), AS (the CI identity), REPO (owner/name),
# ZOMBIE (the forge's ZOMBIE_TASK_TIMEOUT, a Go duration; 10m when unset).
import http.client
import json
import os
import re
import socket
import sys
import time
from datetime import datetime

WORKFLOW = "entry_point.yml"
# how long after its run was made a killed job is still worth running again:
# the forge was gone a whole day in its move of 2026-10-25 (run 900)
WINDOW = 2 * 86400
# attempts a job may have before this leaves it be
ATTEMPTS = 3
KILLED = "context canceled"
# the job that reads every other's result: its failure says another failed
GATE = "ci_ok"
# a task the forge heard nothing from for its zombie timeout (ZOMBIE, as
# the forge is set: 10m unless set otherwise) it marks failed, its log that
# far behind its end at least; most of that is the sign of one lost
LOST_SHARE = 0.8
# the forge stops a task still going after 3 hours (its endless timeout,
# checked every 30 minutes): a hang, not a restart
ENDLESS = (3 * 3600 - 300, 3 * 3600 + 40 * 60)
TS = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.\d+)?Z ?(.*)$")


def stamp(s):
    return datetime.fromisoformat(s + "+00:00").timestamp()


def seconds(d):
    """a Go duration (1h2m3.5s, 4m0s, 250ms) in seconds"""
    total = 0.0
    for n, u in re.findall(r"([\d.]+)(h|ms|m|s|us|µs|ns)", d or ""):
        total += (
            float(n)
            * {
                "h": 3600,
                "m": 60,
                "s": 1,
                "ms": 1e-3,
                "us": 1e-6,
                "µs": 1e-6,
                "ns": 1e-9,
            }[u]
        )
    return total


def killed(log, duration, zombie=600):
    """why a failed job's attempt was killed from outside, or None when it
    failed of itself. log: the attempt's whole log; duration: the forge's
    (its start to its end), in seconds; zombie: the forge's zombie timeout"""
    lines = [m for m in (TS.match(x) for x in log.splitlines()) if m]
    if not lines:
        return None
    if ENDLESS[0] <= duration <= ENDLESS[1]:
        return None
    texts = [m.group(2).strip() for m in lines]
    last = next((t for t in reversed(texts) if t), "")
    if last == KILLED:
        return "its runner stopped under it (context canceled)"
    if any(t.startswith("🏁") for t in texts):
        return None
    first, end = stamp(lines[0].group(1)), stamp(lines[-1].group(1))
    gap = first + duration - end
    if gap >= LOST_SHARE * zombie:
        return (
            "the forge lost it: no word from its runner for %d min before it was marked failed"
            % (gap // 60)
        )
    return None


def mine(run):
    """a pull request's run (opened, pushed to, edited), or main's full
    run, of the CI workflow; never a push to another branch (a member's own
    CI, say), which is not this one's to rerun"""
    if run.get("workflow_id", WORKFLOW) != WORKFLOW:
        return False
    ev = run.get("event") or ""
    return ev.startswith("pull_request") or (
        ev == "push" and run.get("prettyref") == "main"
    )


def candidates(runs, now):
    """the runs to look into: mine, the newest for their ref, made within
    the window"""
    newest = {}
    for r in runs:
        ref = r.get("prettyref")
        if mine(r) and ref and r["index_in_repo"] > newest.get(ref, 0):
            newest[ref] = r["index_in_repo"]
    out = []
    for r in runs:
        if not mine(r) or r["index_in_repo"] != newest.get(r.get("prettyref")):
            continue
        try:
            made = datetime.fromisoformat(r.get("created") or "").timestamp()
        except ValueError:
            continue
        if now - made <= WINDOW:
            out.append(r)
    return out


class UnixConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout):
        super().__init__("localhost", timeout=timeout)
        self.unix = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.unix)


class Forge:
    """forgejo, as the CI identity, over its socket"""

    def __init__(self, sock, actor, repo):
        self.sock, self.actor, self.repo = sock, actor, repo

    def ask(self, method, path, body=None):
        """(status, headers, body bytes)"""
        c = UnixConnection(self.sock, 60)
        try:
            headers = {"X-WEBAUTH-USER": self.actor}
            if body is not None:
                headers["content-type"] = "application/json"
            c.request(method, path, body=body, headers=headers)
            r = c.getresponse()
            return r.status, dict(r.getheaders()), r.read()
        finally:
            c.close()

    def runs(self):
        s, _, b = self.ask("GET", "/api/v1/repos/%s/actions/runs?limit=50" % self.repo)
        if s != 200:
            raise OSError("the forge did not list its runs: http %s" % s)
        return json.loads(b).get("workflow_runs") or []

    def jobs(self, run):
        """[{id, name, status, canRerun, duration}], by their index in the run"""
        s, _, b = self.ask(
            "POST",
            "/%s/actions/runs/%d/jobs/0/attempt/1" % (self.repo, run),
            b'{"logCursors":[]}',
        )
        if s != 200:
            raise OSError("run #%d: its jobs: http %s" % (run, s))
        return json.loads(b)["state"]["run"]["jobs"]

    def attempt(self, run, job):
        """the job's latest attempt, from where the forge sends its page"""
        s, h, _ = self.ask("GET", "/%s/actions/runs/%d/jobs/%d" % (self.repo, run, job))
        m = re.search(r"/attempt/(\d+)$", h.get("Location") or h.get("location") or "")
        if s not in (301, 302, 303, 307, 308) or not m:
            raise OSError("run #%d job %d: no attempt (http %s)" % (run, job, s))
        return int(m.group(1))

    def log(self, run, job, attempt):
        s, _, b = self.ask(
            "GET",
            "/%s/actions/runs/%d/jobs/%d/attempt/%d/logs"
            % (self.repo, run, job, attempt),
        )
        if s != 200:
            raise OSError("run #%d job %d: its log: http %s" % (run, job, s))
        return b.decode(errors="replace")

    def rerun(self, run, job):
        s, _, b = self.ask(
            "POST", "/%s/actions/runs/%d/jobs/%d/rerun" % (self.repo, run, job), b""
        )
        return s, b[:200]


def pass_once(forge, now=None, say=print, zombie=600):
    """look at every run once, rerun what a restart killed; returns the
    (run, job index) pairs it reran"""
    now = time.time() if now is None else now
    done = []
    for r in candidates(forge.runs(), now):
        n = r["index_in_repo"]
        try:
            jobs = forge.jobs(n)
            failed = [
                (i, j) for i, j in enumerate(jobs) if j.get("status") == "failure"
            ]
            if not failed:
                continue
            # someone stopped it (a person, a newer push, a failure's
            # stop-run): not this one's to start again
            if any(j.get("status") == "cancelled" for j in jobs):
                say("run #%d: cancelled; left as it is" % n)
                continue
            kills = []
            for i, j in failed:
                a = forge.attempt(n, i)
                why = killed(forge.log(n, i, a), seconds(j.get("duration")), zombie)
                if why is None and j.get("name") == GATE:
                    continue  # it failed for what it needs, and runs again with them
                if why is None:
                    kills = None  # a real failure: the run stays red
                    say("run #%d: %s failed of itself; left as it is" % (n, j["name"]))
                    break
                kills.append((i, j, a, why))
        except (OSError, http.client.HTTPException, ValueError, KeyError) as e:
            say("run #%d: not read (%s); next time" % (n, e))
            continue
        for i, j, a, why in kills or []:
            if a >= ATTEMPTS:
                say(
                    "run #%d: %s was killed on its attempt %d; left as it is"
                    % (n, j["name"], a)
                )
                continue
            code, said = forge.rerun(n, i)
            say(
                "run #%d (%s): %s - %s; run again -> %s %s"
                % (n, r.get("prettyref"), j["name"], why, code, said)
            )
            if code == 200:
                done.append((n, i))
    return done


def main():
    forge = Forge(os.environ["FORGE"], os.environ["AS"], os.environ["REPO"])
    # the forge down, restarting or on another box is not a failure of
    # this: there is nothing to rerun until it answers here, and the next
    # pass looks again
    try:
        pass_once(
            forge,
            say=lambda s: print(s, flush=True),
            zombie=seconds(os.environ.get("ZOMBIE") or "10m"),
        )
    except (OSError, http.client.HTTPException, ValueError) as e:
        print("the forge is not answering (%s); next time" % e)
    return 0


if __name__ == "__main__":
    sys.exit(main())
