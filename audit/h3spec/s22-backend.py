import http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def _r(self):
        body=b"ok\n"
        self.send_response(200)
        self.send_header("Content-Length",str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self): self._r()
    def do_POST(self):
        l=int(self.headers.get('Content-Length',0)); self.rfile.read(l); self._r()
    def do_HEAD(self):
        self.send_response(200); self.send_header("Content-Length","3"); self.end_headers()
    def log_message(self,*a): pass
socketserver.TCPServer.allow_reuse_address=True
with socketserver.TCPServer(("127.0.0.1",23000),H) as s: s.serve_forever()
