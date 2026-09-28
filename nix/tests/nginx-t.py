# nginx -t on a box's config, with what only exists on the box stood in:
# the certificates, the files units write under /run, /var and /etc, the
# logs and the pid.
import re
import subprocess
import sys

conf, nginx, tmp = sys.argv[1:4]
s = open(conf).read()
s = re.sub(r"/var/lib/acme/[^/]+/(fullchain|chain)\.pem", tmp + "/cert.pem", s)
s = re.sub(r"/var/lib/acme/[^/]+/key\.pem", tmp + "/key.pem", s)
s = re.sub(r"include\s+/(run|var|etc)/[^;]*;", "include " + tmp + "/empty.conf;", s)
s = re.sub(r"/var/(log|cache|spool)/nginx", tmp, s)
s = re.sub(r"^(\s*)pid\s+[^;]*;", r"\1pid " + tmp + "/nginx.pid;", s, flags=re.M)
# the build's default access log is under /var/log, which is not here
s = re.sub(r"^(\s*)http\s*\{", lambda m: m.group(0) + " access_log " + tmp + "/access.log;", s, count=1, flags=re.M)
# -t binds what it listens on, and the build may not have the low ports
s = re.sub(r"(listen\s+(?:\S*:)?)(\d+)", lambda m: m.group(1) + str(int(m.group(2)) + 10000), s)
open(tmp + "/empty.conf", "w").close()
open(tmp + "/t.conf", "w").write(s)
r = subprocess.run([nginx, "-t", "-e", "stderr", "-p", tmp, "-c", tmp + "/t.conf"], capture_output=True, text=True)
print("\n".join(l for l in r.stderr.splitlines() if "[warn]" not in l))
sys.exit(r.returncode)
