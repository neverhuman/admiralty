#!/usr/bin/env python3
"""Static server for the captains dashboard. 127.0.0.1 only; serves index.html and status.json only."""
import http.server, os, sys
BASE = os.environ.get('CDASH_RUNTIME', '/home/ubuntu/xbabe2_kicker/captains-dashboard')  # runtime data dir (status.json, history, tokens.db); code lives in admiralty/dashboard
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18890
ALLOW = {'/': 'index.html', '/index.html': 'index.html', '/status.json': 'status.json'}
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        f = ALLOW.get(self.path.split('?')[0])
        if not f:
            self.send_error(404); return
        try:
            data = open(os.path.join(BASE, f), 'rb').read()
        except Exception:
            self.send_error(503, 'not ready'); return
        self.send_response(200)
        self.send_header('Content-Type', 'application/json' if f.endswith('.json') else 'text/html; charset=utf-8')
        self.send_header('Cache-Control', 'no-store')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers(); self.wfile.write(data)
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(('127.0.0.1', PORT), H).serve_forever()
