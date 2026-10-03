# The unlock Worker's box side (client/mail/worker/unlock.js), the same
# signed messages and the same rule - home is the address or the router it
# last unlocked from - for the vm test. The Worker itself is tested by the
# mail-worker check; this proves the box speaks to it.
import base64, json, threading, time
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature

S = {'keys': {}, 'share': {}, 'home': {}, 'pending': {}, 'approved': {}, 'stolen': set(), 'seen': {}, 'asked': 0, 'refuse_ip': False}
lock = threading.Lock()
unb = lambda s: base64.urlsafe_b64decode(s + '=' * (-len(s) % 4))

def signed(box, msg, sig):
    pem = S['keys'].get(box)
    if not pem:
        return False
    raw = unb(sig)
    try:
        # r||s as WebCrypto takes it, or the DER a TPM may give (unlock.js takes both)
        der = raw if len(raw) != 64 else encode_dss_signature(int.from_bytes(raw[:32], 'big'), int.from_bytes(raw[32:], 'big'))
        serialization.load_der_public_key(base64.b64decode(pem)).verify(der, msg.encode(), ec.ECDSA(hashes.SHA256()))
        return True
    except Exception:
        return False

class H(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *a): pass
    def send(self, v, code=200):
        b = json.dumps(v).encode(); self.send_response(code)
        self.send_header('content-type', 'application/json'); self.send_header('content-length', str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        with lock:
            return self.send({k: (list(v) if isinstance(v, set) else v) for k, v in S.items() if k not in ('keys',)})
    def do_POST(self):
        n = int(self.headers.get('content-length') or 0)
        b = json.loads(self.rfile.read(n) or b'{}')
        ip = '1.2.3.4' if S['refuse_ip'] else self.client_address[0]
        with lock:
            p = self.path
            if p == '/test/box': S['keys'][b['box']] = b['key']; return self.send({'ok': True})
            if p == '/test/away': S['refuse_ip'] = True; S['home'][b['box']] = {'ip': '9.9.9.9', 'mac': '00:00:00:00:00:00'}; return self.send({'ok': True})
            if p == '/test/approve':
                pd = S['pending'][b['box']]; S['approved'][b['box']] = dict(pd); return self.send({'ok': True})
            if p == '/test/stolen':
                (S['stolen'].add if b['stolen'] else S['stolen'].discard)(b['box']); return self.send({'ok': True})
            box, at, mac, sig = b.get('box'), b.get('at'), b.get('mac', ''), b.get('sig', '')
            if abs(time.time() - int(at)) > 600 or int(at) <= S['seen'].get(box, 0):
                return self.send({'error': 'stale'}, 403)
            if p == '/api/unlock/share':
                if not signed(box, f"commonty unlock share v1\0{box}\0{at}\0{b['share']}", sig): return self.send({'error': 'not signed'}, 403)
                S['seen'][box] = int(at)
                if box in S['share']: return self.send({'error': 'kept'}, 409)
                S['share'][box] = b['share']; S['home'][box] = {'ip': ip, 'mac': mac}
                return self.send({'ok': True})
            if p == '/api/unlock':
                S['asked'] += 1
                if not signed(box, f"commonty unlock v1\0{box}\0{at}\0{mac}", sig): return self.send({'error': 'not signed'}, 403)
                S['seen'][box] = int(at)
                if box in S['stolen']: return self.send({'error': 'this box is marked stolen'}, 403)
                h = S['home'].get(box) or {}
                ok = S['approved'].get(box)
                if (ip == h.get('ip') or (mac and mac == h.get('mac'))) or (ok and ok == {'ip': ip, 'mac': mac}):
                    S['home'][box] = {'ip': ip, 'mac': mac}; S['pending'].pop(box, None); S['approved'].pop(box, None)
                    return self.send({'share': S['share'][box]})
                S['pending'][box] = {'ip': ip, 'mac': mac}
                return self.send({'waiting': 'approve'}, 202)
        self.send({}, 404)

ThreadingHTTPServer(('0.0.0.0', 8000), H).serve_forever()
