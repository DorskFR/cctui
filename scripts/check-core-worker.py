#!/usr/bin/env python3
"""Manual Linux smoke: fresh core DB, real daemon/harness, controlled provider.

Requires a built target/debug/cctui-core-server (or CCTUI_TEST_CORE_BINARY), Docker,
and the repository's
PostgreSQL test container on port 5481. Ports 18727 and 18729 must be free.
CCTUI_TEST_WORKER_IMAGE overrides the published worker image used for rolling
compatibility checks. CCTUI_TEST_DAEMON_BINARY optionally mounts a local Linux
daemon in that image. --adapter codex tests Responses; --child also checks real
MCP discovery, Claude child execution, slot retention and archival. The Codex
fixture explicitly uses full access inside its disposable container. No paid provider request is made. Each run removes only
its own database, container and work directory; diagnostic logs are retained.
With --kubeconfig and --worker-url, an existing disposable cluster/profile and
admission webhook run the real sandboxed Job through a published dispatcher.
"""

import argparse
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
import urllib.parse
import urllib.request
import uuid

root = pathlib.Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--adapter", choices=["claude-code", "codex"], default="claude-code"
)
parser.add_argument(
    "--child",
    action="store_true",
    help="Use a real Codex parent and Claude child; verify slots and archival.",
)
parser.add_argument(
    "--kubeconfig",
    help="Explicit disposable Kubernetes kubeconfig; use a real dispatcher/Job.",
)
parser.add_argument(
    "--upgrade-from",
    help="Older core binary for same-database upgrade and rollback acceptance.",
)
parser.add_argument("--worker-url", help="Core URL reachable from Kubernetes workers.")
parser.add_argument(
    "--context-pack",
    action="store_true",
    help="Fetch a real git context pack and verify instructions/prompt reach the harness.",
)
parser.add_argument("--namespace", default="core-acceptance")
parser.add_argument("--profile", default="core-smoke")
args = parser.parse_args()
if args.upgrade_from and (
    args.kubeconfig or args.child or args.context_pack or args.adapter != "claude-code"
):
    parser.error("--upgrade-from requires the simple Claude machine fixture")
if args.kubeconfig and (args.child or not args.worker_url):
    parser.error(
        "Kubernetes requires --worker-url and does not support --child in this smoke"
    )
kube = (
    [
        "kubectl",
        "--kubeconfig",
        str(pathlib.Path(args.kubeconfig).resolve()),
        "-n",
        args.namespace,
    ]
    if args.kubeconfig
    else None
)
job_handle = None
if args.context_pack and not kube:
    parser.error("--context-pack currently requires --kubeconfig")
pack_dir = None
pack_payload = {}
if args.context_pack:
    pack_tmp = tempfile.TemporaryDirectory(prefix="cctui-core-context-")
    pack_dir = pathlib.Path(pack_tmp.name)
    repo = pack_dir / "context.git"
    subprocess.run(
        ["git", "init", "--initial-branch=main", str(repo)],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    (repo / "AGENTS.md").write_text(
        "CORE_CONTEXT_INSTRUCTIONS: This fixture expects a short answer and no tool calls.\n"
    )
    (repo / "prompts").mkdir()
    (repo / "prompts" / "fixture.md").write_text(
        "CORE_CONTEXT_PROMPT: Respond with core-worker-ok. Do not call tools.\n"
    )
    subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "user.signingkey="
            + subprocess.check_output(
                ["git", "-C", str(root), "config", "--get", "user.signingkey"],
                text=True,
            ).strip(),
            "commit",
            "-S",
            "-m",
            "Context acceptance fixture",
        ],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    pack_ref = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True
    ).strip()
    host = urllib.parse.urlsplit(args.worker_url).hostname
    pack_payload = {
        "prompt_file": "fixture.md",
        "env": {
            "CONTEXT_PACK_URL": f"http://{host}:18729/context.git",
            "CONTEXT_PACK_REF": pack_ref,
        },
    }
if args.child:
    args.adapter = "codex"
expected_text = "core-parent-child-ok" if args.child else "core-worker-ok"
daemon_binary = os.environ.get("CCTUI_TEST_DAEMON_BINARY")

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
    def git_backend(self, raw=b""):
        path = urllib.parse.urlsplit(self.path)
        if not pack_dir or not path.path.startswith("/context.git/"):
            self.send_error(404)
            return
        env = dict(
            os.environ,
            GIT_PROJECT_ROOT=str(pack_dir),
            GIT_HTTP_EXPORT_ALL="1",
            PATH_INFO=path.path,
            QUERY_STRING=path.query,
            REQUEST_METHOD=self.command,
            CONTENT_TYPE=self.headers.get("Content-Type", ""),
            CONTENT_LENGTH=str(len(raw)),
            HTTP_GIT_PROTOCOL=self.headers.get("Git-Protocol", ""),
        )
        response = subprocess.run(
            ["git", "http-backend"],
            input=raw,
            env=env,
            capture_output=True,
            check=True,
            timeout=15,
        ).stdout
        headers, body = response.split(b"\r\n\r\n", 1)
        headers = [line.decode().split(":", 1) for line in headers.split(b"\r\n")]
        self.send_response(
            next(
                (
                    int(value.strip().split()[0])
                    for key, value in headers
                    if key.lower() == "status"
                ),
                200,
            )
        )
        for key, value in headers:
            if key.lower() != "status":
                self.send_header(key, value.strip())
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.git_backend()

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
        if self.path.startswith("/context.git/"):
            self.git_backend(raw)
            return
        if self.headers.get("Content-Encoding") == "gzip":
            raw = gzip.decompress(raw)
        body = json.loads(raw)
        assert self.headers.get("Authorization") == "Bearer fixture-upstream-key", (
            "gateway did not forward provider credential"
        )
        if args.context_pack:
            serialized = json.dumps(body)
            assert "CORE_CONTEXT_INSTRUCTIONS" in serialized, (
                "pack instructions absent from harness request"
            )
            assert "CORE_CONTEXT_PROMPT" in serialized, (
                "pack prompt absent from harness request"
            )
        requests.append((self.path, body))
        if "count_tokens" in self.path:
            response = json.dumps({"input_tokens": 10}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(response)
            return
        if body.get("model", "").startswith("claude"):
            message = {
                "id": "msg_fixture",
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "text",
                        "text": "core-child-ok" if args.child else "core-worker-ok",
                    }
                ],
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
                        "delta": {
                            "type": "text_delta",
                            "text": "core-child-ok" if args.child else "core-worker-ok",
                        },
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
                    (
                        "event: " + event + "\ndata: " + json.dumps(data) + "\n\n"
                    ).encode()
                )
            return
        message = {
            "id": "msg_fixture",
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [
                {"type": "output_text", "text": "core-worker-ok", "annotations": []}
            ],
        }
        response = {
            "id": "resp_fixture",
            "object": "response",
            "created_at": 1793230000,
            "status": "completed",
            "model": body.get("model", "gpt-5.5"),
            "output": [message],
            "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15},
        }
        tool_results = [
            item
            for item in body.get("input", [])
            if item.get("type") == "function_call_output"
        ]
        if args.child and not tool_results:
            available = list(body.get("tools", []))
            for item in body.get("input", []):
                if item.get("type") == "tool_search_output":
                    available.extend(item.get("tools", []))
            candidates = []
            for tool in available:
                if tool.get("type") == "namespace":
                    candidates.extend(
                        (nested, tool["name"]) for nested in tool.get("tools", [])
                    )
                else:
                    candidates.append((tool, None))
            agent = next(
                (
                    (tool, namespace)
                    for tool, namespace in candidates
                    if tool.get("name", "").endswith("CctuiAgent")
                ),
                None,
            )
            print(
                "Available tool names:",
                [
                    (tool.get("name", tool.get("type")), namespace)
                    for tool, namespace in candidates
                ],
                flush=True,
            )
            if agent is None:
                message = {
                    "id": "ts_fixture",
                    "type": "tool_search_call",
                    "execution": "client",
                    "call_id": "call_search",
                    "status": "completed",
                    "arguments": {"query": "cctui CctuiAgent", "limit": 1},
                }
            else:
                tool, namespace = agent
                message = {
                    "id": "fc_fixture",
                    "type": "function_call",
                    "call_id": "call_fixture",
                    "name": tool["name"],
                    "arguments": json.dumps(
                        {
                            "adapter": "claude-code",
                            "model": "claude-sonnet-4-5",
                            "prompt": "child-task-smoke: respond with core-child-ok",
                            "permission_mode": "auto",
                            "timeout_secs": 45,
                        }
                    ),
                    "status": "completed",
                }
                if namespace:
                    message["namespace"] = namespace
            response["output"] = [message]
        elif args.child:
            assert "core-child-ok" in json.dumps(tool_results), tool_results
            print("Parent received real child completion", flush=True)
            message["content"][0]["text"] = "core-parent-child-ok"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        events = [
            {
                "type": "response.created",
                "response": dict(response, status="in_progress", output=[]),
            },
            {
                "type": "response.output_item.added",
                "output_index": 0,
                "item": dict(message, status="in_progress", content=[]),
            },
            {
                "type": "response.content_part.added",
                "item_id": message["id"],
                "output_index": 0,
                "content_index": 0,
                "part": {"type": "output_text", "text": "", "annotations": []},
            },
            {
                "type": "response.output_text.delta",
                "item_id": message["id"],
                "output_index": 0,
                "content_index": 0,
                "delta": "core-worker-ok",
            },
            {
                "type": "response.output_text.done",
                "item_id": message["id"],
                "output_index": 0,
                "content_index": 0,
                "text": "core-worker-ok",
            },
            {"type": "response.output_item.done", "output_index": 0, "item": message},
            {"type": "response.completed", "response": response},
        ]
        if message["type"] == "function_call":
            events = [
                {
                    "type": "response.created",
                    "response": dict(response, status="in_progress", output=[]),
                },
                {
                    "type": "response.output_item.added",
                    "output_index": 0,
                    "item": dict(message, arguments="", status="in_progress"),
                },
                {
                    "type": "response.function_call_arguments.delta",
                    "item_id": message["id"],
                    "output_index": 0,
                    "delta": message["arguments"],
                },
                {
                    "type": "response.function_call_arguments.done",
                    "item_id": message["id"],
                    "output_index": 0,
                    "arguments": message["arguments"],
                },
                {
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": message,
                },
                {"type": "response.completed", "response": response},
            ]
        elif message["type"] == "tool_search_call":
            events = [
                {
                    "type": "response.created",
                    "response": dict(response, status="in_progress", output=[]),
                },
                {
                    "type": "response.output_item.added",
                    "output_index": 0,
                    "item": message,
                },
                {
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": message,
                },
                {"type": "response.completed", "response": response},
            ]
        else:
            for event in events:
                if event["type"] == "response.output_text.delta":
                    event["delta"] = expected_text
                if event["type"] == "response.output_text.done":
                    event["text"] = expected_text
        for event in events:
            self.wfile.write(
                (
                    "event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n"
                ).encode()
            )

    def log_message(self, *args):
        pass


provider = http.server.ThreadingHTTPServer(
    ("0.0.0.0" if args.context_pack else "127.0.0.1", 18729), Provider
)
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
        CCTUI_EXTERNAL_URL=args.worker_url or base,
        CCTUI_HOST="0.0.0.0" if kube else "127.0.0.1",
        CCTUI_PORT="18727",
        CCTUI_UPSTREAM_ALLOWED_HOSTS="127.0.0.1:18729",
        RUST_LOG="info",
    )
    with open(log_dir / "server.log", "w") as log:
        server = subprocess.Popen(
            [
                args.upgrade_from
                or os.environ.get(
                    "CCTUI_TEST_CORE_BINARY",
                    str(root / "target/debug/cctui-core-server"),
                )
            ],
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
            "provider": "openai-compatible"
            if args.adapter == "codex"
            else "anthropic-compatible",
            "base_url": "http://127.0.0.1:18729",
            "access_token": "fixture-upstream-key",
        },
    )
    if args.child:
        req(
            "POST",
            "/accounts",
            key,
            {
                "name": "child-provider",
                "provider": "anthropic-compatible",
                "base_url": "http://127.0.0.1:18729",
                "access_token": "fixture-upstream-key",
            },
        )
    if kube:
        enrolled = req(
            "POST",
            "/dispatcher/enroll",
            key,
            {"name": "kube-smoke", "kind": "kubernetes"},
        )
        work = pathlib.Path(tempfile.mkdtemp(prefix="cctui-core-kube-dispatcher-"))
        config = work / "dispatcher.toml"
        config.write_text(
            "server_url = " + json.dumps(base) + "\n"
            "dispatcher_key = " + json.dumps(enrolled["dispatcher_key"]) + "\n"
            "namespace = " + json.dumps(args.namespace) + "\n"
            "default_profile = " + json.dumps(args.profile) + "\n"
            "worker_cctui_url = " + json.dumps(args.worker_url) + "\n"
        )
        config.chmod(0o600)
        subprocess.run(
            [
                "docker",
                "run",
                "--detach",
                "--name",
                worker,
                "--network",
                "host",
                "--user",
                "0:0",
                "-v",
                str(config) + ":/fixture/dispatcher.toml:ro",
                "-v",
                str(pathlib.Path(args.kubeconfig).resolve())
                + ":/fixture/kubeconfig:ro",
                "-e",
                "KUBECONFIG=/fixture/kubeconfig",
                "-e",
                "CCTUI_DISPATCHER_CONFIG=/fixture/dispatcher.toml",
                "-e",
                "RUST_LOG=info",
                os.environ.get(
                    "CCTUI_TEST_DISPATCHER_IMAGE",
                    "ghcr.io/dorskfr/cctui-dispatcher-kube:0.27.0-beta.26",
                ),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        time.sleep(3)
        launched = req(
            "POST",
            "/sessions/dispatch",
            key,
            {
                "dispatcher": "kube-smoke",
                "account": "controlled-provider",
                "timeout": 15,
                "payload": {
                    "profile": args.profile,
                    "adapter": args.adapter,
                    **(
                        pack_payload
                        if args.context_pack
                        else {
                            "prompt": "Respond with core-worker-ok. Do not call tools."
                        }
                    ),
                    "model": "gpt-5.5"
                    if args.adapter == "codex"
                    else "claude-sonnet-4-5",
                },
            },
        )
        job_handle = launched["handle"]
    else:
        machine = req("POST", "/enroll", key, {"hostname": "container-worker"})
        mid = machine["machine_id"]
        sql(
            "INSERT INTO adapters_enabled (machine_id,adapter_id,enabled,config) VALUES ('"
            + mid
            + "','claude-code',true,'{\"mode\":\"oneshot\"}'::jsonb),('"
            + mid
            + "','codex',true,'{}'),('"
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
                *(
                    [
                        "-v",
                        str(pathlib.Path(daemon_binary).resolve())
                        + ":/usr/local/bin/cctui-daemon:ro",
                    ]
                    if daemon_binary
                    else []
                ),
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
                "adapter_id": args.adapter,
                "working_dir": "/work",
                "prompt": "Respond with core-worker-ok. Do not call tools.",
                "account": "controlled-provider",
                "model": "gpt-5.5" if args.adapter == "codex" else "claude-sonnet-4-5",
                "permission_mode": "yolo" if args.adapter == "codex" else "auto",
                **(
                    {
                        "spawn_capability": {
                            "adapters": ["claude-code"],
                            "max_children": 1,
                            "max_depth": 1,
                            "max_permission_mode": "auto",
                        }
                    }
                    if args.child
                    else {}
                ),
            },
        )
    print("Actual daemon accepted spawn:", launched, flush=True)
    sid = launched.get("session_id") or launched["command_id"]
    deadline = time.time() + (150 if kube else 60)
    while True:
        result = req("GET", "/sessions/" + sid + "/result", key)
        if result.get("done"):
            break
        assert time.time() < deadline, ("completion timed out", result)
        time.sleep(0.5)
    assert expected_text in (result.get("final_assistant_text") or ""), result
    assert requests, "provider endpoint never called"
    if args.upgrade_from:
        for label, binary in [
            (
                "upgrade",
                os.environ.get(
                    "CCTUI_TEST_CORE_BINARY",
                    str(root / "target/debug/cctui-core-server"),
                ),
            ),
            ("rollback", args.upgrade_from),
        ]:
            prior_sid = sid
            server.terminate()
            server.wait(timeout=15)
            with open(log_dir / (label + ".log"), "w") as log:
                server = subprocess.Popen(
                    [binary], env=env, stdout=log, stderr=subprocess.STDOUT
                )
            deadline = time.time() + 30
            while True:
                try:
                    version = req("GET", "/version")
                    break
                except urllib.error.URLError:
                    assert server.poll() is None, label + " server exited"
                    assert time.time() < deadline, label + " server startup timed out"
                    time.sleep(0.2)
            assert req("GET", "/me", key), label + " user key lost"
            assert req("GET", "/accounts", key), label + " encrypted account lost"
            saved = req("GET", "/sessions/" + prior_sid + "/result", key)
            assert "core-worker-ok" in (saved.get("final_assistant_text") or ""), (
                label,
                saved,
            )
            deadline = time.time() + 40
            while not req("GET", "/machines/" + mid + "/status", key)["connected"]:
                assert time.time() < deadline, label + " daemon did not reconnect"
                time.sleep(0.3)
            run = req(
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
            sid = run.get("session_id") or run["command_id"]
            deadline = time.time() + 60
            while True:
                result = req("GET", "/sessions/" + sid + "/result", key)
                if result.get("done"):
                    break
                assert time.time() < deadline, (label, result)
                time.sleep(0.3)
            assert "core-worker-ok" in (result.get("final_assistant_text") or ""), (
                label,
                result,
            )
            print(
                "PASS:",
                label,
                version["version"],
                "preserved keys/accounts/results; live daemon reconnected and ran a new real harness task",
                flush=True,
            )
    if args.child:
        parent = sql(
            "SELECT registered_session_id FROM machine_launches WHERE command_id='"
            + launched["command_id"]
            + "'"
        )
        children = sql(
            "SELECT id FROM sessions WHERE parent_id='"
            + parent
            + "' AND adapter_id='claude-code'"
        ).splitlines()
        assert len(children) == 1, children
        token = machine["machine_key"]
        limits = req("GET", "/daemon/sessions/" + parent + "/limits", token)
        assert limits["children"]["used"] == 1, limits
        archived = req(
            "POST",
            "/daemon/sessions/" + parent + "/archive-child",
            token,
            {"session_id": children[0]},
        )
        assert archived["children_used"] == 0 and children[0] in archived["archived"], (
            archived
        )
        assert any(body.get("model", "").startswith("claude") for _, body in requests)
        print(
            "PASS: real child registration retains its slot; archival releases it.",
            flush=True,
        )
    print(
        "PASS: real containerized daemon and harness completed through core gateway:",
        result,
        flush=True,
    )
finally:
    if kube and job_handle:
        # Retain diagnostics and delete only this run's Job, even in a shared namespace.
        name = job_handle.rsplit("/", 1)[-1]
        with open(log_dir / "pods.log", "w") as log:
            for command in [
                ["get", job_handle, "-o", "yaml"],
                [
                    "logs",
                    "-l",
                    "job-name=" + name,
                    "--all-containers=true",
                    "--prefix",
                    "--tail=200",
                ],
                ["delete", job_handle, "--wait=false"],
            ]:
                try:
                    subprocess.run(
                        kube + command,
                        stdout=log,
                        stderr=subprocess.STDOUT,
                        check=False,
                        timeout=20,
                    )
                except subprocess.TimeoutExpired:
                    print("Kubernetes cleanup command timed out:", command, file=log)
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
    if pack_dir:
        shutil.rmtree(pack_dir)
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
