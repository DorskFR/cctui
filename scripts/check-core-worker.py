#!/usr/bin/env python3
"""Manual Linux smoke: fresh core DB, real daemon/harness, controlled provider.

Requires a built target/debug/cctui-core-server, Docker, and the repository's
PostgreSQL test container on port 5481. Ports 18727 and 18729 must be free.
CCTUI_TEST_WORKER_IMAGE overrides the published worker image used for rolling
compatibility checks. No paid provider request is made. Each run removes only
its own database, container and work directory; diagnostic logs are retained.
"""

import gzip
import http.server
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid

root = pathlib.Path(__file__).resolve().parent.parent

db = "cctui_core_worker_" + uuid.uuid4().hex
postgres = os.environ.get("CCTUI_TEST_POSTGRES_CONTAINER", "cctui-postgres-test")
image = os.environ.get("CCTUI_TEST_WORKER_IMAGE", "ghcr.io/dorskfr/cctui-worker:0.26.1")
work = None
server = None
worker = "cctui-core-worker-" + uuid.uuid4().hex[:12]
log_dir = pathlib.Path(tempfile.mkdtemp(prefix="cctui-core-worker-logs-"))
print("Diagnostic logs:", log_dir, flush=True)
base = "http://127.0.0.1:18727"
requests = []


class Provider(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        raw = b""
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            while True:
                size = int(self.rfile.readline().split(b";")[0], 16)
                if size == 0:
                    self.rfile.readline()
                    break
                raw += self.rfile.read(size)
                self.rfile.read(2)
        else:
            raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.headers.get("Content-Encoding") == "gzip":
            raw = gzip.decompress(raw)
        body = json.loads(raw)
        assert self.headers.get("Authorization") == "Bearer fixture-upstream-key", (
            "gateway did not forward provider credential"
        )
        requests.append((self.path, body))
        if "count_tokens" in self.path:
            response = json.dumps({"input_tokens": 10}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(response)
            return
        message = {
            "id": "msg_fixture",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "core-worker-ok"}],
            "model": body.get("model", "claude-sonnet-4-5"),
            "stop_reason": "end_turn",
            "stop_sequence": None,
            "usage": {"input_tokens": 10, "output_tokens": 5},
        }
        self.send_response(200)
        if not body.get("stream"):
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(message).encode())
            return
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        start = dict(
            message,
            content=[],
            stop_reason=None,
            usage={"input_tokens": 10, "output_tokens": 0},
        )
        events = [
            ("message_start", {"type": "message_start", "message": start}),
            (
                "content_block_start",
                {
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""},
                },
            ),
            (
                "content_block_delta",
                {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "core-worker-ok"},
                },
            ),
            ("content_block_stop", {"type": "content_block_stop", "index": 0}),
            (
                "message_delta",
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                    "usage": {"output_tokens": 5},
                },
            ),
            ("message_stop", {"type": "message_stop"}),
        ]
        for event, data in events:
            self.wfile.write(
                ("event: " + event + "\ndata: " + json.dumps(data) + "\n\n").encode()
            )

    def log_message(self, *args):
        pass


provider = http.server.ThreadingHTTPServer(("127.0.0.1", 18729), Provider)
threading.Thread(target=provider.serve_forever, daemon=True).start()


def sql(query):
    return subprocess.check_output(
        ["docker", "exec", postgres, "psql", "-U", "postgres", "-d", db, "-Atc", query],
        text=True,
    ).strip()


def req(method, path, key=None, body=None):
    headers = {"Content-Type": "application/json"}
    if key:
        headers["Authorization"] = "Bearer " + key
    request = urllib.request.Request(
        base + "/api/v1" + path,
        headers=headers,
        method=method,
        data=json.dumps(body).encode() if body is not None else None,
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            raw = response.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raise AssertionError((path, e.code, e.read().decode()))


try:
    subprocess.run(
        ["docker", "exec", postgres, "createdb", "-U", "postgres", db], check=True
    )
    env = os.environ.copy()
    for name in [
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
    ]:
        env.pop(name, None)
    env.update(
        DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5481/" + db,
        CCTUI_ADMIN_TOKENS="fixture-admin",
        CCTUI_VAULT_KEY="42" * 32,
        CCTUI_EXTERNAL_URL=base,
        CCTUI_HOST="127.0.0.1",
        CCTUI_PORT="18727",
        CCTUI_UPSTREAM_ALLOWED_HOSTS="127.0.0.1:18729",
        RUST_LOG="info",
    )
    with open(log_dir / "server.log", "w") as log:
        server = subprocess.Popen(
            [str(root / "target/debug/cctui-core-server")],
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
    deadline = time.time() + 20
    while True:
        try:
            req("GET", "/version")
            break
        except urllib.error.URLError:
            assert time.time() < deadline, "server startup timed out"
            time.sleep(0.2)
    user = req("POST", "/admin/users", "fixture-admin", {"name": "automation-worker"})
    key = user["key"]
    account = req(
        "POST",
        "/accounts",
        key,
        {
            "name": "controlled-provider",
            "provider": "anthropic-compatible",
            "base_url": "http://127.0.0.1:18729",
            "access_token": "fixture-upstream-key",
        },
    )
    machine = req("POST", "/enroll", key, {"hostname": "container-worker"})
    mid = machine["machine_id"]
    sql(
        "INSERT INTO adapters_enabled (machine_id,adapter_id,enabled,config) VALUES ('"
        + mid
        + "','claude-code',true,'{\"mode\":\"oneshot\"}'::jsonb),('"
        + mid
        + "','codex',false,'{}'),('"
        + mid
        + "','opencode',false,'{}')"
    )
    work = pathlib.Path(tempfile.mkdtemp(prefix="cctui-core-worker-"))
    work.chmod(0o777)
    subprocess.run(
        [
            "docker",
            "run",
            "--detach",
            "--rm",
            "--name",
            worker,
            "--network",
            "host",
            "--user",
            "1000:1000",
            "--entrypoint",
            "/usr/local/bin/cctui-daemon",
            "-v",
            str(work) + ":/work",
            "-e",
            "CCTUI_MACHINE_KEY=" + machine["machine_key"],
            "-e",
            "CCTUI_URL=" + base,
            "-e",
            "RUST_LOG=info",
            image,
            "run",
            "--no-auto-update",
        ],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    deadline = time.time() + 40
    while not req("GET", "/machines/" + mid + "/status", key)["connected"]:
        assert time.time() < deadline, "daemon connection timed out"
        time.sleep(0.3)
    time.sleep(2)
    launched = req(
        "POST",
        "/sessions/spawn",
        key,
        {
            "machine_id": mid,
            "adapter_id": "claude-code",
            "working_dir": "/work",
            "prompt": "Respond with core-worker-ok. Do not call tools.",
            "account": "controlled-provider",
            "model": "claude-sonnet-4-5",
            "permission_mode": "auto",
        },
    )
    print("Actual daemon accepted spawn:", launched, flush=True)
    sid = launched["session_id"]
    deadline = time.time() + 60
    while True:
        result = req("GET", "/sessions/" + sid + "/result", key)
        if result.get("done"):
            break
        assert time.time() < deadline, ("completion timed out", result)
        time.sleep(0.5)
    assert "core-worker-ok" in (result.get("final_assistant_text") or ""), result
    assert requests, "provider endpoint never called"
    print(
        "PASS: real containerized daemon + real Claude harness completed through core gateway:",
        result,
        flush=True,
    )
finally:
    with open(log_dir / "daemon.log", "w") as daemon_log:
        subprocess.run(
            ["docker", "logs", worker],
            stdout=daemon_log,
            stderr=subprocess.STDOUT,
            check=False,
        )
    subprocess.run(
        ["docker", "rm", "-f", worker],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if server:
        server.terminate()
        server.wait(timeout=10)
    if work:
        shutil.rmtree(work)
    provider.shutdown()
    subprocess.run(
        [
            "docker",
            "exec",
            postgres,
            "dropdb",
            "-U",
            "postgres",
            "--if-exists",
            "--force",
            db,
        ],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
