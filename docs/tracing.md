# Tracing (OpenTelemetry)

cctui-server, cctui-daemon and the webui export traces over OTLP/HTTP. Every
exporter is off unless configured; removing the configuration below restores
the untraced behaviour.

## Turning it on

| Component | Setting | Notes |
| --- | --- | --- |
| cctui-server, cctui-daemon | `OTEL_EXPORTER_OTLP_ENDPOINT` (or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) | protobuf only; `OTEL_SDK_DISABLED=true` wins. Standard `OTEL_SERVICE_NAME`, `OTEL_TRACES_SAMPLER(_ARG)`, `OTEL_RESOURCE_ATTRIBUTES` apply. Defaults: `cctui-server`, `cctui-daemon`. |
| webui | `window.CCTUI_CONFIG.otel = { endpoint, serviceName?, sampleRatio? }` in `config.js` | Spans POST to `${endpoint}/v1/traces` as OTLP/JSON. Use `'/api/v1/otlp'`. |

The browser never talks to the collector. It posts to the server's
`POST /api/v1/otlp/v1/traces`, which takes the same login as the rest of the
API (the auth cookie rides a same-origin request), caps the body at 1 MiB, and
forwards it verbatim (`Content-Type`, `Content-Encoding`) to the server's own
traces endpoint: `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, else
`OTEL_EXPORTER_OTLP_ENDPOINT` + `/v1/traces`. With the server exporter off it
answers 404; an unreachable or rejecting collector gives 502.

The daemon strips every `OTEL_*` var it inherited from the environment of the
agents it launches, so their processes do not export into the daemon's
collector. A session env that sets an `OTEL_*` var explicitly keeps it.

## Spans of a spawn

One Launch click in the spawn dialog produces a single trace:

```
ui.spawn_session.launch                  webui
└─ HTTP POST (fetch)                     webui, sends traceparent
   └─ POST /api/v1/sessions/spawn        server (http.route=/api/v1/sessions/spawn)
      ├─ db.query …                      server (db.system, db.operation, db.sql.table)
      └─ daemon.command                  daemon, parented through the WS frame
         └─ session.spawn                daemon
```

## End-to-end check

`webui/e2e/otel-spawn-trace.spec.ts` (Playwright project `otel-trace`) clicks
Launch against a live local stack, collects every span in an in-process OTLP
sink, and asserts exactly one trace holds the chain above across three
services. It spawns a real session in `$OTEL_E2E_CWD` (default: the OS temp
dir) and kills it afterwards.

```sh
make local/demo
# a daemon enrolled against the local stack, exporting to the sink:
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 ./cctui-daemon run
make local/otel-trace
```

`make local/otel-trace` recreates `cctui-server` with
`CCTUI_SERVER_OTLP_ENDPOINT=http://host.docker.internal:4318`, builds the
webui, and runs the spec against `vite preview` (which proxies `/api` to
`:8700`). Browser spans take the real path: page → `/api/v1/otlp` → server →
sink. The server image must contain the tracing code (`CCTUI_SERVER_IMAGE`).
A later `make local/up` recreates the server with export off.

The spec skips, never fails, when the server is not reachable, no admin token
is found, port 4318 is taken, or no daemon is online.

| Variable | Default | Purpose |
| --- | --- | --- |
| `OTEL_E2E_API` | `http://localhost:8700` | reachability probe |
| `OTEL_E2E_URL` | the `vite preview` URL | UI under test |
| `OTEL_E2E_SINK_PORT` / `LOCAL_OTLP_PORT` | `4318` | sink port |
| `OTEL_E2E_MACHINE` | first online machine | machine id or name to launch on |
| `OTEL_E2E_CWD` | OS temp dir | working directory of the spawned session |
