# The transcode test: a box plays a file it cannot read at rest.
import json

box.wait_for_unit("dd-transcode.service")
box.wait_for_open_port(4190)
# A clip big enough that ffmpeg must seek for the index at its end, as a
# real file makes it, and long enough that the transcode of it is still
# running when the test ends the session: twenty seconds encodes faster
# than the test can reach the DELETE, which would make "it was killed"
# indistinguishable from "it had already finished".
box.succeed("ffmpeg -hide_banner -loglevel error -f lavfi -i testsrc=duration=120:size=640x360:rate=25 -f lavfi -i sine=frequency=440:duration=120 -c:v mpeg4 -q:v 5 -c:a aac -shortest /tmp/clip.mp4")
# MPEG-4 part 2 is no browser's: this clip is encoded as it plays (H.264 in
# 4:2:0 would only be repacked, done in a second, and the checks below on a
# film still being worked on would have nothing to find)
# the box's key for this run, then the clip in the format a library holds it
# (rclone writes it; the data key is what a device would seal to the box)
key = json.loads(box.succeed("curl -sf http://127.0.0.1:4190/key"))["key"]
box.succeed("mkdir -p /tmp/store && printf '[enc]\\ntype = crypt\\nremote = /tmp/store\\npassword = %s\\npassword2 = %s\\n' \"$(rclone obscure 'a library key')\" \"$(rclone obscure 'a996a28ca51c9cf1d3f8e2038c8339c8')\" > /tmp/rclone.conf")
box.succeed("rclone --config /tmp/rclone.conf copyto /tmp/clip.mp4 enc:clip.mp4")
sealed = box.succeed("ls /tmp/store | head -1").strip()
# The bucket honours Range and the box depends on it: a piece is one
# ranged GET, and the block at that offset is decrypted from where the
# range starts. python3 -m http.server does NOT honour Range - it answers
# 200 with the whole file - so the first block came back as the header
# plus block zero and would not open. rclone serves ranges, as garage
# does, which is what this is standing in for.
box.succeed("(rclone serve http --addr 127.0.0.1:8000 /tmp/store >/dev/null 2>&1 &)")
box.wait_until_succeeds("curl -sf -o /dev/null http://127.0.0.1:8000/", timeout=30)
assert "206" == box.succeed("curl -s -o /dev/null -w '%%{http_code}' -H 'range: bytes=0-9' http://127.0.0.1:8000/%s" % sealed).strip(), "the file server must honour Range"
body = box.succeed("python3 %s /tmp/store/%s 'a library key' a996a28ca51c9cf1d3f8e2038c8339c8 '%s' http://127.0.0.1:8000/%s" % (nix["chunker"], sealed, key, sealed)).strip()
box.succeed("printf '%s' '%s' > /tmp/body.json" % ("%s", body.replace("'", "'\\''")))
started = json.loads(box.succeed("curl -sf -X POST -H 'content-type: application/json' -d @/tmp/body.json http://127.0.0.1:4190/session"))
sid = started["id"]
# the page learns how long the film is, for its "left" and its progress bar
assert started["duration"] and abs(started["duration"] - 120) < 2, started
assert started["from"] == 0, started
# the session is answered once there is something to play: the playlist
# names a segment the moment the page has its answer, and a segment is
# playable bytes
box.succeed("curl -sf http://127.0.0.1:4190/session/%s/index.m3u8 | grep -q '\\.ts'" % sid)
seg = box.succeed("curl -sf http://127.0.0.1:4190/session/%s/index.m3u8 | grep '\\.ts' | head -1" % sid).strip()
box.succeed("curl -sf -o /tmp/seg.ts http://127.0.0.1:4190/session/%s/%s && test $(stat -c %%s /tmp/seg.ts) -gt 1000" % (sid, seg))
# The session id is on ffmpeg's command line for every process to read. It
# opens nothing for them: the plain listener serves its own user alone, and
# this test runs as root, which is someone else.
assert sid in box.succeed("ps -eo args | grep '[f]fmpeg'"), "the id is visible, which is the point"
assert "libx264" in box.succeed("ps -eo args | grep '[f]fmpeg' | grep %s" % sid), "a picture no browser plays is encoded"
code = box.succeed("curl -s -o /dev/null -w '%%{http_code}' http://127.0.0.1:4191/plain/%s" % sid).strip()
assert code == "403", code
# and a session fetches from the libraries bucket and nowhere else: not a
# box on the network, not the box itself
for url in ["http://127.0.0.1:4191/plain/x", "http://169.254.169.254/latest", "http://127.0.0.1:8000.evil/x"]:
    code = box.succeed("curl -s -o /dev/null -w '%%{http_code}' -X POST -H 'content-type: application/json' -d '{\"url\":\"%s\",\"key\":\"AAAA\",\"size\":100}' http://127.0.0.1:4190/session" % url).strip()
    assert code == "400", (url, code)
# a key sealed to some other box is refused
box.fail("curl -sf -X POST -H 'content-type: application/json' -d '{\"url\":\"http://127.0.0.1:8000/x\",\"key\":\"AAAA\",\"size\":100}' http://127.0.0.1:4190/session")
# Resuming: a session asked to begin partway starts ffmpeg there, and says
# where it began so the page can add it back; past the end starts over
box.succeed("python3 -c 'import json; b=json.load(open(\"/tmp/body.json\")); b[\"from\"]=60; json.dump(b, open(\"/tmp/resume.json\", \"w\"))'")
resumed = json.loads(box.succeed("curl -sf -X POST -H 'content-type: application/json' -d @/tmp/resume.json http://127.0.0.1:4190/session"))
assert resumed["from"] == 60, resumed
box.succeed("ps -eo args | grep '[f]fmpeg' | grep %s | grep -q -- '-ss 60.000'" % resumed["id"])
box.succeed("curl -sf -X DELETE http://127.0.0.1:4190/session/%s" % resumed["id"])
box.succeed("python3 -c 'import json; b=json.load(open(\"/tmp/body.json\")); b[\"from\"]=500; json.dump(b, open(\"/tmp/past.json\", \"w\"))'")
past = json.loads(box.succeed("curl -sf -X POST -H 'content-type: application/json' -d @/tmp/past.json http://127.0.0.1:4190/session"))
assert past["from"] == 0, past
box.succeed("curl -sf -X DELETE http://127.0.0.1:4190/session/%s" % past["id"])
# the work is really happening, and ending the session really ends it:
# a viewer who closes a film after a minute must not leave the box
# transcoding the rest of it
box.succeed("pgrep -f '[f]fmpeg.*%s' >/dev/null" % sid)
box.succeed("curl -sf -X DELETE http://127.0.0.1:4190/session/%s" % sid)
box.wait_until_fails("pgrep -f '[f]fmpeg.*%s' >/dev/null" % sid, timeout=30)
# over: the session and its files are gone
box.fail("curl -sf http://127.0.0.1:4190/session/%s/index.m3u8" % sid)
box.fail("test -d /run/dd-transcode/%s" % sid)
# One film at a time per member: tom's second film ends his first, and
# someone else's is left alone
h = "-H 'content-type: application/json' -d @/tmp/body.json http://127.0.0.1:4190/session"
t1 = json.loads(box.succeed("curl -sf -X POST -H 'X-DD-User: tom' " + h))["id"]
s1 = json.loads(box.succeed("curl -sf -X POST -H 'X-DD-User: sarah' " + h))["id"]
t2 = json.loads(box.succeed("curl -sf -X POST -H 'X-DD-User: tom' " + h))["id"]
box.fail("curl -sf http://127.0.0.1:4190/session/%s/index.m3u8" % t1)
box.wait_until_fails("pgrep -f '[f]fmpeg.*%s' >/dev/null" % t1, timeout=30)
box.wait_until_succeeds("curl -sf http://127.0.0.1:4190/session/%s/index.m3u8" % s1, timeout=60)
box.wait_until_succeeds("curl -sf http://127.0.0.1:4190/session/%s/index.m3u8" % t2, timeout=60)
for i in (s1, t2):
    box.succeed("curl -sf -X DELETE http://127.0.0.1:4190/session/%s" % i)
# the demo is many visitors under one name: one's film never ends another's
d1 = json.loads(box.succeed("curl -sf -X POST -H 'X-DD-User: demo' " + h))["id"]
d2 = json.loads(box.succeed("curl -sf -X POST -H 'X-DD-User: demo' " + h))["id"]
box.wait_until_succeeds("curl -sf http://127.0.0.1:4190/session/%s/index.m3u8" % d1, timeout=60)
box.wait_until_succeeds("curl -sf http://127.0.0.1:4190/session/%s/index.m3u8" % d2, timeout=60)
for i in (d1, d2):
    box.succeed("curl -sf -X DELETE http://127.0.0.1:4190/session/%s" % i)
