# The Files page in a browser the app let in: files put in the library
# through the page's own library.js, then search across the library, a
# folder's size, a folder out as one zip, a folder in, an address that opens
# a file, and Scan to PDF with its words read. What the gate holds is looked
# at too: sealed bytes under encrypted names.
import base64, io, json, re, sys, time, urllib.request, zipfile
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
        time.sleep(0.1)  # sleep-ok: the polling loop's own beat between looks
    ok(False, f"waited for {what}")
    return False


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
    ctx = b.new_context(viewport={"width": 1280, "height": 820}, accept_downloads=True)
    # no save-file picker: a zip comes as a download, as in browsers without one
    ctx.add_init_script("delete window.showSaveFilePicker")
    pg = ctx.new_page()
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
    pg.on(
        "console",
        lambda m: (
            errs.append(m.text)
            if m.type == "error" and "404" not in m.text and "405" not in m.text
            else None
        ),
    )
    pg.goto(f"{HOST}/_dd/files")
    pg.wait_for_selector("#drive:not([hidden])", timeout=20000)

    # files in, through the page's own library.js: sealed in the tab
    pg.evaluate(
        """async () => {
          const l = await import('/_dd/static/library.js');
          const lib = (await l.unlock()).ok;
          for (const d of ['Files', 'Files/Lease', 'Files/Lease/Old']) await l.mkdir(lib, d);
          const put = (p, s) => l.put(lib, p, new Blob([s]));
          await put('Files/Lease/lease.pdf', '%PDF-1.4 lease, by hand\\n'.repeat(40));
          await put('Files/Lease/Old/form.txt', 'an old form');
          await put('Files/notes.txt', 'notes');
        }"""
    )
    pg.reload()
    pg.wait_for_selector("#rows tr", timeout=15000)
    dav = get("/test/dav")
    ok(
        len(dav) == 3
        and not any("Lease" in d["path"] or "form" in d["path"] for d in dav),
        "the gate holds three files under encrypted names",
    )

    until(
        lambda: re.search(
            r"\d+ (B|KB)", pg.locator("#rows tr", has_text="Lease").inner_text()
        ),
        "a folder shows what it holds",
    )
    pg.screenshot(path=f"{OUT}/1-files.png")

    # search: a name two folders down, from the top
    pg.fill("#search", "form")
    until(
        lambda: "form.txt" in pg.inner_text("#found"), "found anywhere in the library"
    )
    ok("My files / Lease / Old" in pg.inner_text("#found"), "and where it is")
    pg.screenshot(path=f"{OUT}/2-search.png")
    pg.locator("#found .hit").first.click()
    until(lambda: "Old" in pg.inner_text("#crumbs"), "a found file opens in its folder")
    until(
        lambda: pg.is_visible("#preview") and pg.inner_text("#pv-name") == "form.txt",
        "and is shown",
    )
    pg.click("#pv-close")

    # search stays in the tab: once the names are in, typing asks nothing,
    # and no request ever carries what was typed
    pg.fill("#search", "")
    pg.click('[data-place="files"]')
    pg.wait_for_selector("#rows tr")
    asked = len(get("/test/log"))
    pg.fill("#search", "lease")
    until(
        lambda: "lease.pdf" in pg.inner_text("#found"),
        "found again from the names in the tab",
    )
    log = get("/test/log")
    ok(
        len(log) == asked,
        f"typing a search asked the box nothing ({len(log) - asked} requests)",
    )
    ok(
        not any("lease" in path.lower() or "form" in path.lower() for _, path in log),
        "and no request carries a word searched for",
    )
    pg.fill("#search", "")

    # a rename is found at once by its new name, never its old
    pg.locator("#rows tr", has_text="notes.txt").locator("button.more").click()
    pg.locator(".popmenu button", has_text="Rename").click()
    pg.fill("#ask-input", "renamed notes.txt")
    pg.click("#ask-ok")
    until(lambda: "renamed notes.txt" in pg.inner_text("#rows"), "renamed in the list")
    pg.fill("#search", "notes")
    until(
        lambda: "renamed notes.txt" in pg.inner_text("#rows"), "the new name is found"
    )
    pg.fill("#search", "notes.t")
    ok(
        "notes.txt" not in pg.inner_text("#found").replace("renamed notes.txt", ""),
        "and the old one is not",
    )
    pg.fill("#search", "")

    # a file trashed: the folder above it shows its new size
    size_of = lambda: (
        pg.locator("#rows tr", has_text="Lease").locator("td").nth(3).inner_text()
    )
    before = size_of()
    pg.locator("#rows tr", has_text="Lease").locator("button.open").click()
    pg.wait_for_selector("#rows tr")
    pg.locator("#rows tr", has_text="lease.pdf").locator("button.more").click()
    pg.locator(".popmenu button", has_text="Move to trash").click()
    until(lambda: "lease.pdf" not in pg.inner_text("#rows"), "trashed")
    pg.click('[data-place="files"]')
    until(
        lambda: size_of() not in (before, "—"),
        f"the folder's size follows ({before} before)",
    )
    ok(size_of() == "11 B", f"Lease now holds the old form alone: {size_of()}")

    # a folder out as one zip
    pg.click('[data-place="files"]')
    pg.wait_for_selector("#rows tr")
    pg.locator("#rows tr", has_text="Lease").locator("input[type=checkbox]").check()
    with pg.expect_download() as dl:
        pg.click("#sel-down")
    z = zipfile.ZipFile(io.BytesIO(open(dl.value.path(), "rb").read()))
    names = sorted(z.namelist())
    ok(z.testzip() is None, "the zip reads back whole")
    ok(
        names == ["Lease/", "Lease/Old/", "Lease/Old/form.txt"]
        and z.read("Lease/Old/form.txt") == b"an old form",
        f"the folder and all under it, opened {names}",
    )

    # a folder in: its folders made, nothing climbing out of where it goes
    pg.evaluate(
        """async () => {
          const l = await import('/_dd/static/library.js');
          const m = await import('/_dd/static/files-more.js');
          const lib = (await l.unlock()).ok;
          await m.putAll(lib, 'Files', [
            { dir: true, rel: 'Trip' },
            { rel: 'Trip/day 1/a.txt', file: new File(['first'], 'a.txt') },
            { rel: '../up.txt', file: new File(['x'], 'up.txt') },
          ], () => ({ step() {}, done() {}, fail() {} }));
        }"""
    )
    pg.click('[data-place="files"]')
    until(lambda: "Trip" in pg.inner_text("#rows"), "the folder that went in")
    pg.locator("#rows tr", has_text="Trip").locator("button.open").click()
    until(lambda: "day 1" in pg.inner_text("#rows"), "with its own folder inside")
    ok(
        len(get("/test/dav")) == 4,
        "two files more at the gate, up.txt kept inside Files",
    )

    # an address with a file in it
    other = ctx.new_page()
    other.goto(f"{HOST}/_dd/files#" + "Files%2FLease%2FOld%2Fform.txt")
    until(
        lambda: (
            other.is_visible("#preview") and other.inner_text("#pv-name") == "form.txt"
        ),
        "an address opens the file it names",
    )
    other.close()

    # Scan to PDF: a picture of a page, read into words
    png = pg.evaluate(
        """() => {
          const c = document.createElement('canvas');
          c.width = 1240; c.height = 1754;
          const x = c.getContext('2d');
          x.fillStyle = '#fff'; x.fillRect(0, 0, c.width, c.height);
          x.fillStyle = '#111'; x.font = 'bold 64px DejaVu Sans, sans-serif';
          x.fillText('RESIDENTIAL LEASE', 120, 220);
          x.font = '44px DejaVu Sans, sans-serif';
          x.fillText('Monthly rent is due on the first day.', 120, 340);
          return c.toDataURL('image/png').split(',')[1];
        }"""
    )
    pg.click('[data-place="files"]')
    pg.wait_for_selector("#rows tr")
    pg.click("#scan")
    pg.wait_for_selector("dialog.dd-scan[open]")
    pg.locator("dialog.dd-scan input[type=file]:not([capture])").set_input_files(
        files=[
            {
                "name": "page.png",
                "mimeType": "image/png",
                "buffer": base64.b64decode(png),
            }
        ]
    )
    pg.wait_for_selector("dialog.dd-scan .view canvas", timeout=10000)
    pg.screenshot(path=f"{OUT}/3-scan.png")
    pg.locator("dialog.dd-scan button", has_text="Save PDF").click()
    until(lambda: len(get("/test/dav")) == 5, "the scan saved at the gate", secs=120)
    words = pg.evaluate(
        """async () => {
          const l = await import('/_dd/static/library.js');
          const mu = await import('/_dd/web/mupdf.js');
          const lib = (await l.unlock()).ok;
          const scan = (await l.list(lib, 'Files')).find((e) => /^Scan .*\\.pdf$/.test(e.name));
          const doc = mu.Document.openDocument(await l.fetchPlain(lib, scan.path), 'application/pdf');
          return scan.name + ' | ' + doc.loadPage(0).toStructuredText().asText();
        }"""
    )
    ok(
        "LEASE" in words.upper() and "RENT" in words.upper(),
        f"its words can be found: {words[:80]!r}",
    )

    ok(not errs, f"no errors on the page {errs[:5]}")
    b.close()

if failed:
    sys.exit(f"{len(failed)} failed")
print("all passed")
