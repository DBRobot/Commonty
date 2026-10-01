# The mail Worker end to end, in a browser: change an email through the
# three pages, the line in Settings, and the refusals at its API. Run by
# mail-worker.nix against the real Worker with a fake Cloudflare (fake.py).
import base64, hashlib, json, sys, urllib.request
from playwright.sync_api import sync_playwright
W = 'http://localhost:8787'; F = 'http://127.0.0.1:8899'
b64u = lambda b: base64.urlsafe_b64encode(b).rstrip(b'=').decode()
def sign(challenge, origin='https://mail.commonty.test', rp='commonty.test', flags=5):
    req = urllib.request.Request(F + '/sign', data=json.dumps({'challenge': challenge, 'origin': origin, 'rp': rp, 'flags': flags}).encode(), method='POST', headers={'content-type': 'application/json'})
    return json.load(urllib.request.urlopen(req))
def fake(path, body=None):
    req = urllib.request.Request(F + path, data=json.dumps(body).encode() if body is not None else None, method='POST' if body is not None else 'GET', headers={'content-type': 'application/json'})
    return json.load(urllib.request.urlopen(req))
STUB = """
// the browser's passkey prompt, answered by the made-up member's key
if (!navigator.credentials) Object.defineProperty(navigator, 'credentials', { value: {}, configurable: true });
Object.defineProperty(navigator.credentials, 'get', { configurable: true, value: async (o) => {
  const c = btoa(String.fromCharCode(...new Uint8Array(o.publicKey.challenge))).replace(/\\+/g,'-').replace(/\\//g,'_').replace(/=+$/,'');
  const a = await window.__sign(c);
  const u = (s) => Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')), x => x.charCodeAt(0)).buffer;
  return { rawId: u(a.id), response: { authenticatorData: u(a.authenticatorData), clientDataJSON: u(a.clientDataJSON), signature: u(a.signature) } };
} });
"""
failed = []
def ok(c, m):
    print(('PASS ' if c else 'FAIL ') + m, flush=True)
    if not c: failed.append(m)
with sync_playwright() as p:
    b = p.chromium.launch(headless=True, args=['--mute-audio', '--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu'])
    ctx = b.new_context(viewport={'width': 1000, 'height': 760})
    ctx.expose_function('__sign', lambda c: sign(c))
    ctx.add_init_script(STUB)
    ctx.route('https://files.commonty.test/**', lambda r: r.fulfill(status=200, body='<p>settings</p>', content_type='text/html'))
    pg = ctx.new_page(); errs = []; pg.on('pageerror', lambda e: errs.append(str(e)))
    back = 'https://files.commonty.test/_dd/settings'
    pg.goto(f'{W}/change?name=tester&back={back}')
    pg.click('#pass'); pg.wait_for_selector('#s2:not([hidden])', timeout=10000)
    ok(True, 'passkey accepted, on to the address')
    pg.fill('#email', 'first@example.com'); pg.click('#save')
    pg.wait_for_selector('#s3:not([hidden])', timeout=10000)
    ok(pg.inner_text('#sent') == 'first@example.com', 'check-your-email shows the address')
    st = fake('/state'); ok('first@example.com' in st['sent'], 'a confirmation went out')
    ok(any(r['actions'][0]['value'] == ['first@example.com'] and r['matchers'][0]['value'] == 'tester@commonty.test' for r in st['rules'].values()), 'rule forwards tester@ to it')
    fake('/verify', {'email': 'first@example.com'})
    pg.wait_for_url('**/_dd/settings?email=changed', timeout=15000); ok(True, 'moved on by itself once opened -> ' + pg.url)
    # the email line, signed in here already
    pg.goto(f'{W}/row?name=tester&back={back}'); pg.wait_for_function("document.getElementById('status').textContent !== 'Checking…'")
    ok(pg.inner_text('#addr') == 'first@example.com' and pg.inner_text('#status') == 'Confirmed', 'email line shows address: ' + pg.inner_text('.er'))
    # a stranger's browser: no address until the passkey
    ctx2 = b.new_context(); ctx2.expose_function('__sign', lambda c: sign(c)); ctx2.add_init_script(STUB)
    p2 = ctx2.new_page(); p2.goto(f'{W}/row?name=tester&back={back}'); p2.wait_for_function("document.getElementById('status').textContent !== 'Checking…'")
    ok(p2.is_hidden('#addr') and p2.is_visible('#show') and p2.inner_text('#status') == 'Confirmed', 'without passkey: status only, Show button')
    p2.click('#show'); p2.wait_for_selector('#addr:not([hidden])', timeout=10000); ok(p2.inner_text('#addr') == 'first@example.com', 'Show reveals after passkey')
    # change to an address already confirmed elsewhere in the account: no wait
    fake('/verify', {'email': 'first@example.com'})
    pg.goto(f'{W}/change?name=tester&back={back}'); pg.wait_for_selector('#s2:not([hidden])', timeout=10000); ok(True, 'signed in a moment ago: straight to the address')
    pg.fill('#email', 'second@example.com'); pg.click('#save'); pg.wait_for_selector('#s3:not([hidden])')
    st = fake('/state'); ok('first@example.com' not in st['addrs'], 'the old address is removed from the account')
    n0 = fake('/state')['sent'].count('second@example.com'); pg.click('#again'); pg.wait_for_timeout(800); ok(fake('/state')['sent'].count('second@example.com') == n0 + 1, 'send it again sends again')
    ok(not errs, f'no errors on the pages {errs}')
    # the API refuses what it should
    import urllib.error
    def call(path, body=None, origin='http://localhost:8787', cookie=None):
        h = {'content-type': 'application/json', 'origin': origin}
        if cookie: h['cookie'] = cookie
        req = urllib.request.Request(W + path, data=json.dumps(body).encode() if body is not None else None, method='POST' if body is not None else 'GET', headers=h)
        try:
            r = urllib.request.urlopen(req); return r.status, json.load(r), r.headers.get('set-cookie')
        except urllib.error.HTTPError as e:
            t = e.read()
            try: return e.code, json.loads(t), None
            except Exception: return e.code, {'raw': t[:120]}, None
    ok(call('/api/me')[0] == 401, 'no session: no address')
    ok(call('/api/email', {'email': 'x@example.com'})[0] == 401, 'no session: no change')
    r = call('/api/challenge', {'name': 'tester'}, origin='http://127.0.0.1:8787'); ok(r[0] == 403, f'another origin cannot call it {r[:2]}')
    r = call('/api/challenge', {'name': 'nobody'}); ok(r[0] == 403, f'not a member: refused {r[:2]}')
    s, c, _ = call('/api/challenge', {'name': 'tester'})
    ok(call('/api/login', {'token': c['token'], 'id': 'cGsx', 'assertion': sign('AAAA')})[0] == 403, 'answer to another challenge: refused')
    ok(call('/api/login', {'token': c['token'], 'id': 'cGsx', 'assertion': sign(c['challenge'], origin='https://evil.example', rp='evil.example')})[0] == 403, 'made for another site: refused')
    ok(call('/api/login', {'token': c['token'], 'id': 'cGsx', 'assertion': sign(c['challenge'], flags=4)})[0] == 403, 'nobody present: refused')
    ok(call('/api/login', {'token': c['token'] + 'x', 'id': 'cGsx', 'assertion': sign(c['challenge'])})[0] == 403, 'tampered challenge token: refused')
    s, _, ck = call('/api/login', {'token': c['token'], 'id': 'cGsx', 'assertion': sign(c['challenge'])}); ok(s == 200 and ck, 'a right answer signs in')
    sess = ck.split(';')[0]
    ok(call('/api/me', cookie=sess)[1].get('email') == 'second@example.com', 'session reads the address')
    forged = sess[:-2] + ('AA' if not sess.endswith('AA') else 'BB')
    ok(call('/api/me', cookie=forged)[0] == 401, 'forged session: refused')
    # a box replaying an old entry after a newer one was seen
    fake('/entry', {'v': 4}); ok(call('/api/challenge', {'name': 'tester'})[0] == 200, 'newer entry v4 taken')
    fake('/entry', {'v': 3}); ok(call('/api/challenge', {'name': 'tester'})[0] == 200, 'old v3 replayed: still served from pinned v4')
    b.close()
if failed:
    sys.exit(f'{len(failed)} failed')
print('all passed')
