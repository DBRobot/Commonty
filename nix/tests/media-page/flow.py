# Movies & TV in a browser the app let in: a film's band starts the box on
# it, so Play has a playlist at once though the box takes two seconds to
# pack the first piece (mail-page/fake.py BOX_SECS); and no session is left
# going at the box once the band and the player are closed.
import json, sys, time, urllib.request
from playwright.sync_api import sync_playwright

PORT = int(sys.argv[1])
OUT = sys.argv[2]
HOST = f"http://s-0123456789abcdef0123456789abcdef.localhost:{PORT}"
F = f"http://127.0.0.1:{PORT}"

failed = []


def ok(c, m):
    print(("PASS " if c else "FAIL ") + m, flush=True)
    if not c:
        failed.append(m)


def get(path):
    return json.load(urllib.request.urlopen(F + path))


def until(f, what, secs=20):
    end = time.time() + secs
    while time.time() < end:
        try:
            if f():
                return True
        except Exception:
            pass
        time.sleep(0.05)  # sleep-ok: the polling loop's own beat between looks
    ok(False, f"waited for {what}")
    return False


def sessions():
    return get("/test/sessions")


def playlists():
    return [p for m, p in get("/test/log") if m == "GET" and p.endswith("/index.m3u8")]


with sync_playwright() as p:
    b = p.chromium.launch(
        headless=True,
        args=[
            "--mute-audio",
            "--no-sandbox",
            "--disable-dev-shm-usage",
            "--disable-gpu",
        ],
    )
    pg = b.new_context(viewport={"width": 1280, "height": 820}).new_page()
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
    pg.goto(f"{HOST}/_dd/media")
    pg.wait_for_selector("#home:not([hidden])", timeout=20000)

    # two films in, through the page's own library.js
    pg.evaluate(
        """async () => {
          const l = await import('/_dd/static/library.js');
          const lib = (await l.unlock()).ok;
          await l.mkdir(lib, 'Movies');
          await l.put(lib, 'Movies/First Film (2020).mkv', new Blob(['a film'.repeat(500)]));
          await l.put(lib, 'Movies/Second Film (2021).mkv', new Blob(['another'.repeat(500)]));
        }"""
    )
    pg.reload()
    pg.wait_for_selector("#films .card", timeout=15000)
    first = pg.locator("#films .card", has_text="First Film")
    second = pg.locator("#films .card", has_text="Second Film")

    # the band opens: the box is asked to start at once, before Play
    first.click()
    pg.wait_for_selector("#ribbon:not([hidden])")
    until(lambda: len(sessions()) == 1, "the band asks the box for the film")
    ok(not playlists(), "and nothing is played yet")
    until(
        lambda: all(s["answered"] for s in sessions().values()), "the box has it ready"
    )
    pg.screenshot(path=f"{OUT}/1-band.png")

    # Play: the session the band started, its playlist at once
    t = time.time()
    pg.click("#r-play")
    until(lambda: playlists(), "Play asks for the playlist", secs=10)
    took = time.time() - t
    ok(took < 1.0, f"Play had its playlist in {took:.2f}s, not the box's {2.0:.0f}s")
    ok(len(sessions()) == 1, "and asked the box for no second session")
    pg.screenshot(path=f"{OUT}/2-playing.png")

    # closing the player ends its session; the band, open again, starts one
    # for the same film; closing the band ends that one too
    pg.click("#p-close")
    pg.wait_for_selector("#player", state="hidden")
    until(lambda: len(sessions()) == 2, "the band again asks for the film")
    pg.click("#r-close")
    until(
        lambda: all(s["ended"] for s in sessions().values()),
        "closing the band ends what it started",
    )

    # a band closed before the box answers: what it asked for is still ended
    second.click()
    pg.wait_for_selector("#ribbon:not([hidden])")
    until(lambda: len(sessions()) == 3, "the second film's band asks the box")
    pg.click("#r-close")
    until(
        lambda: all(s["answered"] and s["ended"] for s in sessions().values()),
        "a band closed while the box starts ends the session once it is answered",
    )

    ok(not errs, f"no page errors ({errs[:3]})")
    b.close()

if failed:
    print(f"{len(failed)} failed")
    sys.exit(1)
