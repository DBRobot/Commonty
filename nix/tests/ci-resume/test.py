#!/usr/bin/env python3
# ci-resume.py against a made-up forgejo on a unix socket. The rerun route
# does what forgejo 15's does (services/actions/rerun.go): the job again at
# its next attempt, and every job that needs it, transitively, again too -
# blocked until it can run, any still going cancelled first - but never the
# job's own matrix siblings; a job not done yet is refused.
#
#   test.py CI-RESUME.PY     (CI_OK: ci_ok's step script, from the workflow)
import http.server
import importlib.util
import json
import os
import re
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading
from datetime import datetime, timezone

spec = importlib.util.spec_from_file_location("ci_resume", sys.argv[1])
cr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cr)

REPO = "fleet/commonty"
CI = "commonty-ci"
NOW = datetime(2026, 10, 26, 22, 30, tzinfo=timezone.utc).timestamp()
DONE = {"success", "failure", "cancelled", "skipped"}


def iso(t):
    return datetime.fromtimestamp(t, timezone.utc).isoformat()


def log(start, lines, end=None):
    """a log as the forge keeps it: a timestamp before every line, one a
    second from start; end, when given, is the job's end line"""
    out = []
    t = start
    for x in lines + ([end] if end else []):
        out.append(
            datetime.fromtimestamp(t, timezone.utc).strftime(
                "%Y-%m-%dT%H:%M:%S.1234567Z "
            )
            + x
        )
        t += 1
    return "\n".join(out) + "\n"


T0 = NOW - 3600
OK_LOG = log(T0, ["Runner received task", "building ..."], "🏁  Job succeeded")
# the runner stopped under it (runs 771, 786)
CANCELED = log(
    T0,
    [
        "building '/nix/store/x-vm-test-run-thanos.drv'...",
        "⚙️ [runner]: this step has been cancelled: ctx: context canceled, exec: RUN signal: killed",
        "🏁  Job failed",
        "context canceled",
    ],
)
# the forge went away mid-step: no end line, marked failed when it came back (run 900)
LOST = log(T0, ["Runner received task", "control-e2e> PASS powered off"])
# a test that failed: the job ended itself
REAL = log(
    T0,
    ["FAIL the gate refused it", "error: builder for '/nix/store/x.drv' failed"],
    "🏁  Job failed",
)
# a test whose output says context canceled before it failed of itself
SAYS_IT = log(
    T0,
    ["control-e2e> FAIL context canceled", "error: builder failed"],
    "🏁  Job failed",
)
# a full disk on the runner: failed in its first second, no end line (run 850)
DISK = log(
    T0,
    [
        "Runner received task 20013 of job app_checks",
        "mkdir /var/lib/gitea-runner/node2/.cache/act/a1: no space left on device",
    ],
)
# a runner killed before it sent a line: the forge has no log file, cannot
# move it at the task's end, and its log route answers 500 (runner-restart)
NO_FILE = "(no log file)"


def job(name, status, logtext=None, needs=(), job_id=None, duration="5m0s", attempt=1):
    return {
        "name": name,
        "job_id": job_id or name.split(" ")[0],
        "needs": list(needs),
        "status": status,
        "attempt": attempt,
        "logs": {attempt: logtext} if logtext else {},
        "duration": duration,
    }


ALL = ["files_modified", "vm_tests", "rust"]


def pr_jobs(b_status="failure", b_log=CANCELED, b_duration="5m0s", b_attempt=1):
    return [
        job("files_modified", "success", OK_LOG),
        job("vm_tests (a)", "success", OK_LOG, ["files_modified"]),
        job(
            "vm_tests (b)",
            b_status,
            b_log,
            ["files_modified"],
            duration=b_duration,
            attempt=b_attempt,
        ),
        job("rust", "success", OK_LOG, ["files_modified"]),
        job("ci_ok", "failure", OK_LOG, ALL),
    ]


class Fake:
    def __init__(self):
        self.runs = []
        self.reruns = []
        self.refuse = set()

    def add(self, index, jobs, event="pull_request", ref="#7", **kw):
        r = {
            "id": 1000 + index,
            "index_in_repo": index,
            "event": event,
            "prettyref": ref,
            "workflow_id": kw.get("workflow", "entry_point.yml"),
            "status": kw.get("status", "failure"),
            "created": iso(kw.get("created", NOW - 7200)),
            "commit_sha": "c0ffee",
        }
        self.runs.append((r, jobs))
        return jobs

    def run(self, n):
        return next(j for r, j in self.runs if r["index_in_repo"] == n)

    def rerun(self, n, i):
        jobs = self.run(n)
        target = jobs[i]
        if target["status"] not in DONE or (n, i) in self.refuse:
            return 400
        ids = {target["job_id"]}
        again = [target]
        for j in jobs:
            if j["job_id"] in ids:
                continue
            if any(x in ids for x in j["needs"]):
                again.append(j)
                ids.add(j["job_id"])
        for j in again:
            if j["status"] not in DONE:
                j["status"] = "cancelled"
            j["attempt"] += 1
            j["status"] = "waiting" if j is target else "blocked"
        self.reruns.append((n, i))
        return 200


class Handler(http.server.BaseHTTPRequestHandler):
    fake = None

    def reply(self, code, body=b"", headers=()):
        self.send_response(code)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        # a redirect's client reads the headers and goes
        if body:
            self.wfile.write(body)

    def handle_one(self, method):
        f = Handler.fake
        if self.headers.get("X-WEBAUTH-USER") != CI:
            return self.reply(403)
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n) if n else b""
        p = self.path
        if method == "GET" and p == f"/api/v1/repos/{REPO}/actions/runs?limit=50":
            return self.reply(
                200, json.dumps({"workflow_runs": [r for r, _ in f.runs]}).encode()
            )
        m = re.fullmatch(rf"/{REPO}/actions/runs/(\d+)/jobs/(\d+)(.*)", p)
        if not m:
            return self.reply(404)
        run, i, rest = int(m.group(1)), int(m.group(2)), m.group(3)
        jobs = f.run(run)
        if method == "POST" and rest == "/attempt/1":
            assert json.loads(body) == {"logCursors": []}, body
            view = [
                {
                    "id": k,
                    "name": j["name"],
                    "status": j["status"],
                    "canRerun": j["status"] in DONE,
                    "duration": j["duration"],
                }
                for k, j in enumerate(jobs)
            ]
            return self.reply(
                200, json.dumps({"state": {"run": {"jobs": view}}}).encode()
            )
        if method == "GET" and rest == "":
            return self.reply(
                307,
                headers=[
                    (
                        "Location",
                        f"/{REPO}/actions/runs/{run}/jobs/{i}/attempt/{jobs[i]['attempt']}",
                    )
                ],
            )
        a = re.fullmatch(r"/attempt/(\d+)/logs", rest)
        if method == "GET" and a:
            text = jobs[i]["logs"].get(int(a.group(1)))
            if text == NO_FILE:
                return self.reply(500)
            return self.reply(200, text.encode()) if text else self.reply(404)
        if method == "POST" and rest == "/rerun":
            code = f.rerun(run, i)
            return self.reply(
                code, b'{"redirect":"x"}' if code == 200 else b'{"errorMessage":"no"}'
            )
        return self.reply(404)

    def do_GET(self):
        self.handle_one("GET")

    def do_POST(self):
        self.handle_one("POST")

    def log_message(self, *_):
        pass


class Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

    def get_request(self):
        s, _ = super().get_request()
        return s, ("local", 0)


tmp = tempfile.mkdtemp()
sock = os.path.join(tmp, "forgejo.sock")
srv = Server(sock, Handler)
threading.Thread(target=srv.serve_forever, daemon=True).start()
forge = cr.Forge(sock, CI, REPO)
fails = 0


def case(name, setup, want, after=None):
    """setup(fake) builds the forge's runs; want: the (run, job) pairs a pass
    must rerun; after(fake), when given, checks the state the reruns left"""
    global fails
    fake = Fake()
    Handler.fake = fake
    setup(fake)
    said = []
    got = cr.pass_once(forge, now=NOW, say=said.append)
    ok = sorted(got) == sorted(want) and sorted(fake.reruns) == sorted(want)
    if ok and after:
        ok = after(fake, said)
    print(("ok   " if ok else "FAIL ") + name)
    if not ok:
        fails += 1
        print("     reran", got, "forge saw", fake.reruns, "wanted", want)
        for s in said:
            print("     |", s)


def needs_of(jobs, ci_ok):
    """what the runner hands ci_ok: each job it needs, by its id, from its
    latest attempt; a matrix job fails if any of its entries did"""
    out = {}
    for jid in jobs[ci_ok]["needs"]:
        st = [j["status"] for j in jobs if j["job_id"] == jid]
        r = (
            "failure"
            if "failure" in st
            else "cancelled"
            if "cancelled" in st
            else "success"
            if "success" in st
            else "skipped"
        )
        out[jid] = {"result": r, "outputs": {}}
    return out


def ci_ok_passes(jobs):
    r = subprocess.run(
        ["bash", os.environ["CI_OK"]],
        env={
            **os.environ,
            "NEEDS": json.dumps(needs_of(jobs, len(jobs) - 1)),
            "DRAFT": "false",
        },
        capture_output=True,
        text=True,
    )
    return r.returncode == 0, r.stdout


def killed_alone_then_ci_ok(fake, said):
    jobs = fake.run(5)
    # the killed job runs again, and ci_ok after it; nothing else moves
    if [j["status"] for j in jobs] != [
        "success",
        "success",
        "waiting",
        "success",
        "blocked",
    ]:
        return False
    if [j["attempt"] for j in jobs] != [1, 1, 2, 1, 2]:
        return False
    # before the rerun ends ci_ok's script fails; the rerun passes, and so does it
    passed, out = ci_ok_passes(
        [dict(j, status="failure") if j["name"] == "vm_tests (b)" else j for j in jobs]
    )
    if passed:
        return False
    jobs[2]["status"] = "success"
    jobs[2]["logs"][2] = OK_LOG
    passed, out = ci_ok_passes(jobs)
    if not passed:
        print(out)
        return False
    jobs[4]["status"] = "success"
    # and a second pass has nothing to do
    return cr.pass_once(forge, now=NOW, say=said.append) == []


case(
    "a job its runner stopped under it runs again alone, ci_ok after it, and passes",
    lambda f: f.add(5, pr_jobs(), event="pull_request_sync"),
    [(5, 2)],
    killed_alone_then_ci_ok,
)
case(
    "a job the forge lost while it was away a day runs again",
    lambda f: f.add(5, pr_jobs(b_log=LOST, b_duration="24h9m22s")),
    [(5, 2)],
)
case(
    "a job the forge lost, with what needs it left blocked, runs again with them",
    lambda f: f.add(
        5,
        pr_jobs(b_log=LOST, b_duration="14m2s")[:4]
        + [job("ci_ok", "blocked", None, ALL)],
    ),
    [(5, 2)],
    lambda f, _: (
        [j["status"] for j in f.run(5)]
        == ["success", "success", "waiting", "success", "blocked"]
    ),
)
case(
    "while the rest of the run still goes",
    lambda f: f.add(
        5,
        [
            job("files_modified", "success", OK_LOG),
            job("vm_tests (a)", "running", None, ["files_modified"]),
            job("vm_tests (b)", "failure", LOST, ["files_modified"], duration="14m2s"),
            job("rust", "waiting", None, ["files_modified"]),
            job("ci_ok", "blocked", None, ALL),
        ],
        status="running",
    ),
    [(5, 2)],
    lambda f, _: (
        [j["status"] for j in f.run(5)]
        == ["success", "running", "waiting", "waiting", "blocked"]
    ),
)
case(
    "a job killed beside one that failed for real: the run stays red",
    lambda f: f.add(
        5,
        pr_jobs()[:4]
        + [
            job("lint", "failure", REAL, ["files_modified"]),
            job("ci_ok", "failure", OK_LOG, ALL),
        ],
    ),
    [],
)
case(
    "a run someone cancelled after a job was killed is left",
    lambda f: f.add(
        5,
        pr_jobs()[:4]
        + [
            job("vm_tests (c)", "cancelled", None, ["files_modified"]),
            job("ci_ok", "cancelled", None, ALL),
        ],
        status="cancelled",
    ),
    [],
)
case(
    "a job that failed for real is never run again",
    lambda f: f.add(5, pr_jobs(b_log=REAL)),
    [],
)
case(
    "nor one whose test said context canceled and then failed of itself",
    lambda f: f.add(5, pr_jobs(b_log=SAYS_IT)),
    [],
)
case(
    "nor one that failed at once on a full disk",
    lambda f: f.add(5, pr_jobs(b_log=DISK, b_duration="1.2s")),
    [],
)
case(
    "one killed before its first line, which the forge has no log for, is run again",
    lambda f: f.add(5, pr_jobs(b_log=NO_FILE, b_duration="10m14s")),
    [(5, 2)],
    lambda f, said: any("not one line" in s for s in said),
)
case(
    "but not one with no log that the forge did not wait its zombie timeout on",
    lambda f: f.add(5, pr_jobs(b_log=NO_FILE, b_duration="1.2s")),
    [],
)
case(
    "nor one the forge stopped for running past three hours",
    lambda f: f.add(5, pr_jobs(b_log=LOST, b_duration="3h12m0s")),
    [],
)
case(
    "nor one already on its third attempt",
    lambda f: f.add(5, pr_jobs(b_attempt=3)),
    [],
)
case(
    "a run a newer push superseded is left; the newer one is looked at",
    lambda f: (f.add(5, pr_jobs()), f.add(6, pr_jobs(b_log=REAL))),
    [],
)
case(
    "main's full run is run again; another branch's push and another workflow are not",
    lambda f: (
        f.add(5, pr_jobs(), event="push", ref="main"),
        f.add(6, pr_jobs(), event="push", ref="doorway"),
        f.add(7, pr_jobs(), ref="#8", workflow="members.yml"),
    ),
    [(5, 2)],
)
case(
    "a run made more than two days ago is left",
    lambda f: f.add(5, pr_jobs(), created=NOW - 3 * 86400),
    [],
)


def refused(f):
    f.add(5, pr_jobs())
    f.refuse.add((5, 2))


case("a rerun the forge refuses is not counted, and is tried next time", refused, [])

# the forge not there at all (on another box, or restarting): next time, exit 0
os.environ.update(FORGE=os.path.join(tmp, "gone.sock"), AS=CI, REPO=REPO)
if cr.main() != 0:
    print("FAIL a forge that is not there is not a failure")
    fails += 1
else:
    print("ok   a forge that is not there is next time, not a failure")

srv.shutdown()
shutil.rmtree(tmp)
print(f"{fails} failed" if fails else "every case holds")
sys.exit(1 if fails else 0)
