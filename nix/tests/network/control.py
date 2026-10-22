# A stand-in for dd-control on 127.0.0.1:4195, for the network test's gate in
# agreed mode (nix/tests/network.py): /identity answers the members in the
# file named on the command line, at the fleet's current agreed time, as
# dd-control does; the directory log and the computers are empty.
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STATE = sys.argv[1]


class Control(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/identity":
            s = json.load(open(STATE))
            s["time"] = int(time.time() * 1000)
            s.setdefault("changed", s["height"])
            body = s
        elif path == "/directory":
            body = {"items": [], "next": 0, "height": 1}
        elif path == "/computers":
            body = {"changed": 1, "owners": {}, "outcomes": {}}
        else:
            self.send_error(404)
            return
        b = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)


ThreadingHTTPServer(("127.0.0.1", 4195), Control).serve_forever()
