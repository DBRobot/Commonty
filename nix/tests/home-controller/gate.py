# A stand-in for the gate's controller routes, on the controller itself in
# the vm test: the backup's urls, and the upload kept in a file. The real
# routes are servers/gate/src/controller.rs, tested there.
import http.server
import json

STORE = "/var/lib/test-gate/backup"


class Gate(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        if (
            self.path != "/_dd/controller/backup"
            or "x-dd-controller" not in self.headers
        ):
            self.send_response(404)
            self.end_headers()
            return
        body = json.dumps(
            {"put": "http://127.0.0.1:9000/store", "get": "http://127.0.0.1:9000/store"}
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_PUT(self):
        data = self.rfile.read(int(self.headers.get("content-length") or 0))
        with open(STORE, "wb") as f:
            f.write(data)
        self.send_response(200)
        self.end_headers()

    def do_GET(self):
        self.send_response(404)
        self.end_headers()


http.server.ThreadingHTTPServer(("127.0.0.1", 9000), Gate).serve_forever()
