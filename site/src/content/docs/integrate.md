---
title: Integrate
description: Drive cctui from another system. Dispatch a run, poll or receive its result, and trigger presets with signed webhooks.
---

cctui can be driven from outside by a CI job, a ticket tracker, n8n or your own orchestrator. There are two ways in:

- **Dispatch API.** Your system holds a cctui key and starts a worker run itself. It then polls for the result or receives it on a callback URL.
- **Signed webhooks.** A third party that holds no cctui key posts to a webhook URL. The body is signed with the webhook's secret, and each accepted call queues a run on a launch preset.

Every path below is under `/api/v1` on your server. The examples use `https://cctui.example.org`.

## Dispatch a run

Create an API key that has the `dispatch` scope (Settings › Users & keys) and send it as a bearer token:

```sh
curl -sS https://cctui.example.org/api/v1/sessions/dispatch \
  -H "Authorization: Bearer $CCTUI_KEY" \
  -H 'Content-Type: application/json' \
  -d '{
        "dispatcher":    "kube",
        "session_id":    "ticket-42-attempt-1",
        "account":       "claude",
        "timeout":       60,
        "notify_url":    "https://orchestrator.example.org/cctui/outcome",
        "notify_secret": "a-long-random-string",
        "payload":       { "profile": "worker", "prompt": "Fix ticket 42 and open a PR." }
      }'
```

| Field | Meaning |
|---|---|
| `dispatcher` | The enrolled dispatcher that runs the worker. |
| `session_id` | Optional idempotency key, minted when absent. If you send the same id again, the server answers `deduplicated` and starts nothing new, so retrying is safe. |
| `timeout` | Minutes. Absent means the runtime default. |
| `account` / `accounts` / `provider` | The account whose gateway the worker's model traffic goes through. `accounts` lists at most one per provider family and takes precedence. |
| `notify_url`, `notify_secret` | Where the server posts the outcome, and the key it signs it with. Never logged. |
| `reply_url` | A bearer URL the worker itself posts its result to. Never logged or stored. |
| `payload` | Handed to the worker as is. The [worker contract](https://github.com/DorskFR/cctui/blob/main/docs/worker-contract.md) lists the fields a worker reads. |

The answer is `202`:

```jsonc
{
  "session_id":     "ticket-42-attempt-1",
  "dispatcher":     "kube",
  "handle":         "jobs/cctui-worker-…",
  "status":         "queued",     // dispatched | deduplicated | redispatched | queued
  "queue_position": 2             // only while queued
}
```

`queued` means the worker waits behind its profile's concurrency limit. If you send the same `session_id` again while it waits, you get `queued` and its current position again.

## Poll for the result

```sh
curl -sS https://cctui.example.org/api/v1/sessions/ticket-42-attempt-1/result \
  -H "Authorization: Bearer $CCTUI_KEY"
```

While the dispatch is queued, the answer is `{"done": false, "queued": true, "queue_position": 2}`. After that it is `404` until the worker registers the session, then:

```jsonc
{
  "done":                 true,
  "end_reason":           "completed",   // null while running
  "final_assistant_text": "Opened PR 12. All checks pass.",
  "message_id":           "msg_…",
  "token_usage":          { "tokens_in": 0, "tokens_out": 0, "cost_usd": 0.0,
                            "cache_read_tokens": 0, "cache_creation_tokens": 0 }
}
```

Poll until `done` is `true`. `final_assistant_text` fills in while the session runs, so you can show progress. `GET /sessions/{id}/conversation` returns the full transcript, and `POST /sessions/{id}/kill` stops the worker.

## Receive the outcome

With `notify_url` set, you don't have to poll. The server posts the outcome when the run ends, including when the worker crashes or is lost:

```jsonc
{
  "kind":       "outcome",
  "task_id":    "<payload.task_id, else the session id>",
  "session_id": "ticket-42-attempt-1",
  "status":     "success",       // success | needs_input | failed
  "error":      null,            // the headline when status is failed
  "message":    "Opened PR 12\n\nAll checks pass.",
  "outcome": {
    "status":   "result",        // result | needs_input | failed
    "headline": "Opened PR 12",
    "body":     "All checks pass.",
    "evidence": ["https://github.com/acme/widgets/pull/12"]
  }
}
```

A worker that dies before it declares an outcome still produces `{ "task_id", "status": "failed", "error" }`.

The body is signed with `notify_secret`. The header is `X-CCTUI-Signature: sha256=<hex>`, where the hex is the HMAC-SHA256 of the raw body. Check it before you trust the body, and compare in constant time. Answer with any `2xx`. Any other answer, or no answer, is retried with backoff for over two hours.

Branch on the structured fields (`status`, `outcome.status`), never on the prose.

## Signed webhooks

A webhook connects a third-party sender to a **launch preset**. The preset holds the target (a machine or a dispatcher), the profile and the base prompt. The sender only reports what happened.

Create webhooks on the Webhooks page, or through the API with a `dispatch`-scope key:

```jsonc
// POST /webhooks
{
  "preset_id":           "…",
  "name":                "issues labelled",
  "prompt_template":     "{{event}} {{payload.action}}: {{payload.issue.title}}",
  "rate_per_minute":     30,     // 1–600, default 30
  "max_usd":             5.0,    // optional dollar cap; null for none
  "budget_window_hours": 24,     // 1–720, default 24
  "enabled":             true
}
```

The answer carries the webhook, including its `url`, and the signing secret. This is the only time the secret is shown. `POST /webhooks/{id}/secret` rotates it, and the old secret stops working at once. The `url` is built by the server from its configured external URL, so you can paste it straight into another product's webhook form.

The preset must accept external sources: webhook bodies are text from outside. A machine preset with external sources on must run in `ask` or `auto` mode. The webhook form offers to turn external sources on for a preset that has them off.

### Sending a delivery

Send `POST /api/v1/hooks/{id}` with a JSON object body of at most 256 KiB. It carries no cctui token; the signature is the only credential:

```sh
body='{"action":"labeled","issue":{"number":42,"title":"Crash on start"}}'
sig=$(printf '%s' "$body" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" | sed 's/^.* //')
curl -sS https://cctui.example.org/api/v1/hooks/$WEBHOOK_ID \
  -H 'Content-Type: application/json' \
  -H "X-Hub-Signature-256: sha256=$sig" \
  -H 'X-Event: issues' \
  -H 'X-Delivery-Id: 8f1c2d' \
  -d "$body"
```

This is the scheme GitHub uses for its webhooks. A GitHub webhook works with no glue in between once its content type is set to `application/json`.

| Header | Role |
|---|---|
| `X-Hub-Signature-256` | `sha256=` and the hex HMAC-SHA256 of the raw body under the webhook secret. Required. |
| `X-GitHub-Delivery`, `X-Delivery-Id` or `Idempotency-Key` | The delivery id, used to skip replays. Optional. |
| `X-GitHub-Event` or `X-Event` | The event name, available to the template as `{{event}}`. Optional. |

The two unsigned headers are reduced to `A-Za-z0-9._-` and 64 characters before anything uses them.

### Answers

| Status | Body | Meaning |
|---|---|---|
| `202` | `{"status": "dispatched", "event_id": "…"}` | Queued on the preset's `default` lane. |
| `200` | `{"status": "duplicate"}` | The delivery id or a byte-identical body was already dispatched in the last 30 days. Nothing is queued. To dispatch again, vary the body, for example with a timestamp. |
| `401` | | Missing, malformed or wrong signature. |
| `400` | | The body is not a JSON object. |
| `404` | | Unknown or disabled webhook, a paused preset, or a preset that no longer accepts external sources. |
| `413` | | The body is over 256 KiB. |
| `429` | | More than `rate_per_minute` deliveries in the last minute, or the webhook is over its dollar cap. |

### The prompt

The rendered template is the event the run receives. On a machine preset, the session's prompt is the preset prompt, a blank line, then the batch of events. `{{payload.<path>}}` reads the body: dots walk objects and numbers index arrays, as in `{{payload.issue.labels.0.name}}`. `{{event}}` and `{{delivery}}` come from the headers. A missing field renders empty, a value is cut at 2000 characters, and any other `{{…}}` is left as written. On a machine preset, the raw body is also attached to the session as `webhook-<delivery id>.json`.

### Dollar cap

Set `max_usd` to limit what a webhook can spend. Before a delivery is dispatched, the server adds up the cost of the sessions that this webhook's deliveries reached in the last `budget_window_hours`, priced the same way as the session list. If the total is at or above the cap, the delivery is refused with `429` and logged as `over_budget`, with the amount spent in its detail. Cost is counted as sessions use tokens, so a run already in flight can take the total past the cap. Deliveries are accepted again once older sessions move out of the window.

### Delivery log

`GET /webhooks/{id}/deliveries` returns the newest 100 deliveries. Each has an outcome: `dispatched` (with the session once one launches), `duplicate`, `rejected`, `rate_limited` or `over_budget`, plus a detail line. Up to 60 refusals a minute are logged, and refusals are kept for a day. The same log is shown under each webhook on the Webhooks page.

## Worked example: an orchestrator

An orchestrator that hands tickets to cctui and waits for a PR can use one dispatch per attempt. Here is a minimal version in Python, using only the standard library:

```python
import hashlib, hmac, json, os, urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

BASE = "https://cctui.example.org/api/v1"
KEY = os.environ["CCTUI_KEY"]
NOTIFY_SECRET = os.environ["NOTIFY_SECRET"].encode()

def dispatch(ticket: int, attempt: int) -> str:
    body = {
        "dispatcher": "kube",
        "session_id": f"ticket-{ticket}-attempt-{attempt}",
        "notify_url": "https://orchestrator.example.org/cctui/outcome",
        "notify_secret": NOTIFY_SECRET.decode(),
        "payload": {"profile": "worker", "task_id": f"ticket-{ticket}",
                    "prompt": f"Fix ticket {ticket} and open a PR."},
    }
    req = urllib.request.Request(
        f"{BASE}/sessions/dispatch", data=json.dumps(body).encode(), method="POST",
        headers={"Authorization": f"Bearer {KEY}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req) as res:
        return json.load(res)["session_id"]

class Outcome(BaseHTTPRequestHandler):
    def do_POST(self):
        raw = self.rfile.read(int(self.headers["Content-Length"]))
        want = "sha256=" + hmac.new(NOTIFY_SECRET, raw, hashlib.sha256).hexdigest()
        if not hmac.compare_digest(want, self.headers.get("X-CCTUI-Signature", "")):
            self.send_response(401)
            self.end_headers()
            return
        event = json.loads(raw)
        if event.get("outcome", {}).get("status") == "result":
            print("done:", event["task_id"], event["outcome"]["headline"])
        elif event["status"] == "needs_input":
            print("needs a human:", event["task_id"], event["message"])
        else:
            print("failed:", event["task_id"], event["error"])
        self.send_response(204)
        self.end_headers()

if __name__ == "__main__":
    dispatch(42, 1)
    HTTPServer(("", 8080), Outcome).serve_forever()
```

Because the session id is derived from the ticket and the attempt number, a crash or retry in the orchestrator never starts a second worker for the same attempt. To retry after a failure, increase the attempt number. If the orchestrator can't accept inbound calls, drop `notify_url` and poll `GET /sessions/{id}/result` until `done` instead.

When the trigger is another product's webhook and not your own code, skip the orchestrator: create a preset and a webhook, and paste the webhook's URL and secret into the sender's settings.
