# Cloudflare's Email Routing API and the directory, as the mail Worker uses
# them, and a signer answering a challenge with the made-up member's passkey
# (tester.json). For the end-to-end check (mail-worker.nix); nothing real.
import base64, hashlib, json, sys, threading
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
FIX = json.load(open(sys.argv[1]))
state = {'entry': FIX['v3'], 'rules': {}, 'addrs': {}, 'sent': [], 'n': 0}
key = serialization.load_der_private_key(base64.b64decode(FIX['pkcs8']), None)
b64u = lambda b: base64.urlsafe_b64encode(b).rstrip(b'=').decode()
lock = threading.Lock()

class H(BaseHTTPRequestHandler):
    # kept open, as Cloudflare does: the Worker reuses connections, and one
    # this end had closed under it was "Network connection lost" now and then
    protocol_version = 'HTTP/1.1'
    disable_nagle_algorithm = True
    def log_message(self, *a): pass
    def send(self, v, code=200):
        b = json.dumps(v).encode(); self.send_response(code)
        self.send_header('content-type', 'application/json'); self.send_header('content-length', str(len(b))); self.end_headers(); self.wfile.write(b)
    def body(self):
        n = int(self.headers.get('content-length') or 0); return json.loads(self.rfile.read(n) or b'{}')
    def do_GET(self):
        p = self.path.split('?')[0]
        if p.startswith('/dir/'):
            return self.send(state['entry']) if p == '/dir/tester' else self.send({}, 404)
        if p == '/client/v4/zones': return self.send({'success': True, 'result': [{'id': 'Z', 'account': {'id': 'A'}}]})
        if p == '/client/v4/zones/Z/email/routing/rules': return self.send({'success': True, 'result': list(state['rules'].values()) if 'page=1' in self.path else []})
        if p == '/client/v4/accounts/A/email/routing/addresses': return self.send({'success': True, 'result': list(state['addrs'].values()) if 'page=1' in self.path else []})
        if p == '/state': return self.send({'rules': state['rules'], 'addrs': state['addrs'], 'sent': state['sent']})
        self.send({}, 404)
    def do_POST(self):
        p = self.path; b = self.body()
        with lock:
            if p == '/client/v4/accounts/A/email/routing/addresses':
                e = b['email']
                if e in state['addrs']: return self.send({'success': False, 'errors': [{'message': 'exists'}]}, 409)
                state['n'] += 1; state['addrs'][e] = {'id': f'd{state["n"]}', 'tag': f'd{state["n"]}', 'email': e, 'verified': None}
                state['sent'].append(e); return self.send({'success': True, 'result': state['addrs'][e]})
            if p == '/client/v4/zones/Z/email/routing/rules':
                state['n'] += 1; r = dict(b, id=f'r{state["n"]}'); state['rules'][r['id']] = r; return self.send({'success': True, 'result': r})
        if p == '/verify':  # the person opening the link
            state['addrs'][b['email']]['verified'] = '2026-10-01T00:00:00Z'; return self.send({'ok': True})
        if p == '/entry':  # the directory moving to another version
            state['entry'] = FIX[f'v{b["v"]}']; return self.send({'ok': True})
        if p == '/sign':
            auth = hashlib.sha256(b['rp'].encode()).digest() + bytes([b.get('flags', 5)]) + bytes([0, 0, 0, 1])
            client = json.dumps({'type': 'webauthn.get', 'challenge': b['challenge'], 'origin': b['origin']}).encode()
            sig = key.sign(auth + hashlib.sha256(client).digest(), ec.ECDSA(hashes.SHA256()))
            return self.send({'id': 'cGsx', 'authenticatorData': b64u(auth), 'clientDataJSON': b64u(client), 'signature': b64u(sig)})
        self.send({}, 404)
    def do_PUT(self):
        b = self.body(); rid = self.path.rsplit('/', 1)[1]
        state['rules'][rid] = dict(b, id=rid); self.send({'success': True, 'result': state['rules'][rid]})
    def do_DELETE(self):
        aid = self.path.rsplit('/', 1)[1]
        for e, a in list(state['addrs'].items()):
            if a['id'] == aid: del state['addrs'][e]
        self.send({'success': True})

ThreadingHTTPServer(('127.0.0.1', 8899), H).serve_forever()
