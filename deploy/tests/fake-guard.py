"""Stand-in for cctui-guard's /check in the shim tests.

argv: RESPONSE_FILE LOG_FILE PORT_FILE. The response file's first line is the
HTTP status (or "hang" to never answer), the rest is the body. Each request is
appended to the log as "<path> <body>".
"""

import http.server
import pathlib
import sys
import time

resp_file, log_file, port_file = map(pathlib.Path, sys.argv[1:4])


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        with log_file.open("ab") as log:
            log.write(self.path.encode() + b" " + body + b"\n")
        status, _, payload = resp_file.read_bytes().partition(b"\n")
        if status.strip() == b"hang":
            time.sleep(30)
            return
        self.send_response(int(status))
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
server.daemon_threads = True
tmp = port_file.with_suffix(".tmp")
tmp.write_text(str(server.server_address[1]))
tmp.rename(port_file)
server.serve_forever()
