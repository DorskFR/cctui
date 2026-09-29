# Runtime plugins

A plugin is a bundle an admin installs into the instance. It can contribute a
webui pane (an ES module the SPA imports at runtime), Claude Code skills for
the user's agent sessions, and per-user settings that reach those sessions as
environment variables. Nothing is baked into the images and nothing is written
into the user's repository.

## Install and enable

Two gates, both required before a user sees anything:

1. **Instance** — an admin installs the plugin and turns it on in
   Settings → Plugins (the admin-only **Manage** block at the top of the page).
   `GET /api/v1/plugins` only ever returns
   instance-enabled plugins.
2. **User** — each user flips their own switch in Settings → Plugins. The
   per-plugin settings form only renders once that switch is on.

Installed plugins live in the `plugins` table: the manifest plus the archive in
the existing blob store, extracted into memory. No writable volume is needed,
so a plugin survives a pod restart and reaches every replica: each pod reloads
when the table changes, checked every 10 s and before it answers a session
launch, the plugin lists, or a static file it does not have.

`CCTUI_PLUGINS_DIR=/path/to/plugins` stays as an optional **read-only** source
for dev and the local stack. Those plugins list as "from directory", are always
instance-enabled, and cannot be uninstalled; an installed plugin with the same
id shadows the directory copy. The directory is scanned at startup and
`POST /api/v1/plugins/rescan` re-reads it without a restart. Invalid plugins are
logged and skipped.

The local stack (`deploy/local`) mounts `deploy/local/plugins` read-only at
`/plugins` and already proxies `/plugins/` to the server.

### Archives

An install takes a gzipped tar, either as a JSON `{"url": "https://…"}`
(https only, no internal hosts) or as the `file` part of a multipart upload.
The archive may have the files at the root or under a single folder whose name
equals the manifest `id`. Limits: 5 MB compressed, 20 MB extracted, 500 entries.
Entries that are absolute, escape the root, or are symlinks are refused, and
`plugin.json` is validated exactly as a directory plugin is. Installing a
different version of an existing id upgrades it in place.

## Published catalog

`plugins/catalog.json` in this repo lists the plugins we publish, each pinned to
an exact release asset:

```json
{
  "version": 1,
  "plugins": [
    {
      "id": "yubisashi",
      "name": "Review",
      "description": "Point at your running app and hand comments to the agent",
      "version": "0.5.2",
      "url": "https://github.com/DorskFR/yubisashi/releases/download/v0.5.2/yubisashi-0.5.2.tgz",
      "sha256": "3aa31bb529f23a306854f2282192b8597904268855627c58048d7b162ff072ac",
      "homepage": "https://github.com/DorskFR/yubisashi"
    }
  ]
}
```

Settings → Plugins shows these under **Available** with one button each:
*Install*, *Update to X* when the catalog is ahead of the installed version, or
*Installed*. A catalog install is still off for the instance until the admin
flips its switch.

`POST /api/v1/admin/plugins` with `{"catalog": "<id>"}` resolves the `url` and
`sha256` from the **server's** catalog — a client-supplied url is ignored — and
refuses the archive unless its bytes hash to the pinned digest. The `url` must
point at an archive in the layout above; an npm tarball does not work, because
its files sit under `package/`.

Publishing a new version is a PR that bumps `version`, `url` and `sha256`. The
release workflow re-downloads every entry and re-checks its digest
(`scripts/check-plugin-catalog.sh`), so a wrong hash never reaches a release.

`CCTUI_PLUGIN_CATALOG_URL` says where the server reads the catalog from.
Unset — the normal case — it is the raw `plugins/catalog.json` on the repo's
`main` branch (`CCTUI_REPO` picks the repo), so a new plugin appears without a
cctui release. The answer is cached for an hour, the fetch goes through the same
SSRF guard as a URL install, and a copy embedded at build time serves offline
instances. `CCTUI_PLUGIN_CATALOG_URL=off` uses only that embedded copy.

## Folder layout

```
<dir>/<id>/plugin.json
<dir>/<id>/web/index.js              # ES module, optional
<dir>/<id>/web/*                     # any assets it references
<dir>/<id>/skills/<name>/SKILL.md    # optional, one folder per skill
```

`plugin.json`:

```json
{
  "id": "yubisashi",
  "name": "Review",
  "description": "Point at the UI and comment",
  "version": "0.3.0",
  "cctuiApi": 1,
  "icon": "eye",
  "web": "web/index.js",
  "page": { "title": "Review", "icon": "eye" },
  "styles": ["web/app.css"],
  "skills": ["yubisashi"],
  "settings": [
    { "key": "host", "label": "Bind address", "env": "YUBI_HOST", "type": "string" }
  ]
}
```

- `id` must match `[a-z0-9-]{1,40}` and equal the folder name.
- `cctuiApi` must be `1`; other majors are refused.
- `web` and every `skills` entry are relative paths inside the folder;
  `skills/<name>/SKILL.md` must exist.
- `page` (optional): a full-page surface at `/apps/<id>`, `{ "title": "Review",
  "icon": "eye" }`. `title` must be non-empty; `icon` is an optional Tsumikit
  icon name for the nav entry. A `page` **requires `web`** — the module is what
  exports the page component — and a manifest with one but not the other is
  refused at install. The module must export `page`.
- `styles` (optional): stylesheets the host loads globally alongside the `web`
  bundle, as plugin-folder-relative paths (`["web/app.css"]`). Each is validated
  exactly like `web`: relative, inside the plugin folder (no `..`, no absolute
  path, no hidden segment) and it must exist. They are served by the same
  `GET /plugins/{id}/{path}` static route as everything else. The host links each one once per
  document, with the bundle's `?v=`, before it imports the module. Component CSS
  needs none of this — it is injected at mount.
- `settings` (optional): each entry declares a per-user string value. `env`
  must match `^[A-Z][A-Z0-9_]{0,63}$` and may not be a reserved name
  (`PATH`, `HOME`, `SHELL`, `USER`, `NODE_OPTIONS`, `LD_*`, `DYLD_*`,
  `ANTHROPIC_*`, `CLAUDE_*`, `CCTUI_*`, `OPENAI_*`, `FIREWORKS_*`, `*_PROXY`,
  and the daemon's own contract vars).
- `instanceSettings` (optional): see below.
- `backend` (optional): see [Plugin backends](#plugin-backends).

## Instance settings (admin-owned)

`settings` is per user. `instanceSettings` is per *instance*: one value for the
whole deployment, writable only by an admin.

```json
"instanceSettings": [
  { "key": "upstream", "label": "Backend URL", "type": "url" },
  { "key": "webhookSecret", "label": "Webhook secret", "type": "string", "secret": true }
]
```

- `key` matches `[a-zA-Z][a-zA-Z0-9_-]{0,39}`, is unique, and at most 32 entries.
- `type` is `"string"` or `"url"`. A `url` value must parse as an absolute
  `http`/`https` URL with a host.
- `secret: true` values are sealed with the server's vault key
  (`CCTUI_VAULT_KEY`, the same ChaCha20-Poly1305 vault as API keys and OAuth
  tokens) and are **never returned by any endpoint** — not to users, not to the
  admin who wrote them. The admin API reports only whether each is set.
- Values are at most 2048 characters.

They live in the `plugin_settings` table (migration 155), keyed by plugin id
rather than on the `plugins` row, so a read-only `CCTUI_PLUGINS_DIR` plugin —
which has no `plugins` row — can still be configured. Uninstalling a plugin
deletes its row, secret and all.

### Admin endpoints

`GET /api/v1/admin/plugins/{id}/settings` → `PluginInstanceSettings`:

```json
{
  "id": "ghreview",
  "instance_settings": [ { "key": "upstream", "label": "Backend URL", "type": "url", "secret": false } ],
  "values": { "upstream": "https://ghreview.dorsk.dev" },
  "secrets_set": { "webhookSecret": true },
  "backend_upstream_setting": "upstream",
  "proxy_secret_set": true
}
```

`values` carries non-secret values only; every declared secret appears in
`secrets_set` as a boolean.

`PUT /api/v1/admin/plugins/{id}/settings` with `{"values": { "<key>": "<value>" }}`
returns the same shape. It is a **patch**: keys present are written, a key set to
`""` is cleared, omitted keys keep their value. An undeclared key, a bad `url`,
or an over-long value is a `400`. An unknown plugin id is a `404`.

### What users see

`GET /api/v1/plugins` adds, per plugin:

- `instanceSettings` — the declarations, for display.
- `instanceSettingValues` — non-secret values, and only to a caller who has the
  plugin enabled. Secrets are filtered out twice: once when the values are read
  and again when the response is built.
- `backend` — `true` when the plugin declares one.

It also passes the page surface through:

- `page` — the manifest's `{ title, icon }` verbatim, or `null`.
- `styles` — each manifest entry resolved to `/plugins/<id>/<path>?v=<sha8>`,
  reusing the `web` bundle's content hash so a plugin upgrade busts the CSS cache
  with the module. Empty when the manifest declares none.

## Plugin backends

A plugin may ship its own HTTP service, deployed separately from cctui. The
browser never talks to it directly and never holds a token for it: every call
goes through cctui, which authenticates the user as usual and then asserts that
identity to the backend with a signature.

```json
"instanceSettings": [ { "key": "upstream", "label": "Backend URL", "type": "url" } ],
"backend": { "upstreamSetting": "upstream" }
```

`upstreamSetting` must name a declared instance setting of type `url` that is
**not** secret — the admin has to be able to see and edit it. A manifest that
breaks either rule is refused at install.

### The route

```
ANY /api/v1/plugins/{id}/backend/{*path}
```

- Authenticates with the normal cctui **cookie or bearer** (`read` scope). No
  new credential exists, so nothing is minted into the browser.
- `404` when the plugin is unknown, instance-disabled, or declares no `backend` —
  the three are deliberately indistinguishable. `403` when the caller has not
  enabled the plugin for themselves.
- `503` when no admin has set the upstream, or the plugin has no proxy secret.
- `502` when the upstream does not answer.
- Request and response bodies are **streamed**, never buffered, and the client
  used for upstreams has **no response timeout**, so a `text/event-stream`
  response stays open as long as the backend keeps it open. The request body
  limit is lifted on this route only.
- **No redirects.** The upstream client's redirect policy is `none`: a `3xx` is
  relayed to the caller verbatim rather than followed, so a compromised backend
  cannot walk the proxy to an address the admin never configured.
- The upstream is validated as an absolute `http`/`https` URL with a host, but
  it is **not** put through the SSRF guard — cluster-internal hosts like
  `http://ghreview.cctui.svc.cluster.local:8790` are exactly the point, and only
  an admin can set it.
- **CSRF.** The route sits under the same `/api/v1` gate as everything else, so
  an unsafe method (`POST`/`PUT`/`PATCH`/`DELETE`) carried by the `cctui_auth`
  cookie needs an allowed `Origin` (else `Referer`). Bearer calls are unaffected.

### What the backend receives

Stripped before forwarding: `Authorization`, the `cctui_auth` and
`cctui_preview` cookies (other cookies are kept, per cookie, not per header),
`Host`, every hop-by-hop header, and **every inbound `X-Cctui-*` header**, so a
caller cannot supply its own identity.

Injected:

| Header | Value |
| --- | --- |
| `X-Cctui-User-Id` | the authenticated user's UUID |
| `X-Cctui-User-Name` | `users.name` |
| `X-Cctui-Plugin` | the plugin id |
| `X-Cctui-Ts` | Unix seconds when the proxy signed |
| `X-Cctui-Sig` | lowercase hex `HMAC-SHA256(secret, canonical)` |

The canonical string is, byte for byte:

```
<METHOD> LF <PATH> LF <TS> LF <USER_ID>
```

i.e. `format!("{method}\n{path}\n{ts}\n{user_id}")`. Exactly:

- `METHOD` is the HTTP method upper-case, as received (`GET`, `POST`, …).
- `PATH` is the `{*path}` suffix with **exactly one leading slash and no query
  string or fragment** — `v1/pulls?state=open` signs as `/v1/pulls`. The query
  is still forwarded, it is simply not signed.
- `TS` is the Unix timestamp in seconds, decimal, no padding.
- `USER_ID` is the UUID in lower-case hyphenated form.
- There is no trailing newline.

A backend verifies by recomputing this with its own copy of the secret, in
constant time, and rejecting a `TS` outside its clock-skew window (ghreview
allows ±300 s). Fixed vectors both sides' tests read live in
[`plugin-proxy-signature-vectors.json`](./plugin-proxy-signature-vectors.json) —
change the signer and that test fails on both sides.

### The proxy secret

One secret per plugin, sealed in `plugin_settings.proxy_secret` with the server's
vault key. It is minted when a plugin declaring a `backend` is installed, and the
install response carries it **once** as `proxy_secret` (only when freshly
minted). `POST /api/v1/admin/plugins/{id}/proxy-secret` rotates it and returns
the new value once. No other endpoint ever returns it; `proxy_secret_set` only
says whether one exists. Rotating it breaks the backend until its
`GHREVIEW_PROXY_SECRET` (or equivalent) is updated — rotate both together.

### SSE and replicas

The proxy is a **stateless per-pod forward**. It holds no shared state: the pod
that receives the browser's request opens its own connection to the upstream and
streams bytes through it. There is no cross-pod hop as previews have, and none is
needed — any replica can serve any plugin backend request, and an SSE stream
simply lives for as long as that one pod↔upstream connection does. A rolling
restart drops open streams, and the client is expected to reconnect
(`EventSource` does so by itself). If the *upstream* runs several replicas, it is
responsible for its own fan-out; cctui does not broadcast between them.

### Admin UI

Settings › Plugins, **Manage** block: a row that declares `instanceSettings` or a
`backend` gets a **Configure** button, and the form is only fetched while it is
open. Non-secret fields show their stored value; a secret field is write-only —
always blank, typed values are sent, and a badge says "set" or "not set" with a
**Clear** action that writes `""`. Only keys the admin actually typed are sent, so
leaving a secret blank keeps it.

A plugin with a backend also shows the proxy secret's state and a **Rotate**
button. The secret itself is displayed exactly once, in the response that mints
it — on rotation, and on an install that created one (`AdminPluginInfo.proxy_secret`).
Copy it then or rotate again.

### Host context (`HostContext`)

The host sets a Svelte context under `HOST_CONTEXT_KEY` above every mounted
surface — pane and page alike. v1.1 (contract major 1, minor 1):

```ts
interface HostContext {
	cctuiApi: number;                 // 1
	cctuiApiMinor?: number;           // 1
	origin: string;                   // the webui origin
	user?: { id: string; name: string; isAdmin: boolean };
	apiFetch?(path: string, init?: RequestInit): Promise<Response>;
	pluginFetch?(path: string, init?: RequestInit): Promise<Response>;
	navigate?(path: string): void;
	openSpawn?(req: { prompt: string; working_dir?: string; machine_id?: string }): void;
	toast?(message: string, tone?: 'ok' | 'info' | 'error'): void;
}
```

Every member past `origin` is optional in the type, because an older host does
not have it: a plugin checks before calling (`ctx.toast?.(…)`). `user` is filled
in once `GET /me` answers. `apiFetch` prefixes `/api/v1`; `pluginFetch` prefixes
`/api/v1/plugins/<id>/backend` for the plugin's own id, so the upstream URL and
its shared secret stay on the server. `openSpawn` opens the host's New session
form pre-filled, wherever the user is — the plugin never launches a session
itself, the user submits the form.

### Security model

**A plugin is admin-trusted code that runs with the user's session.** There is no
sandbox, and there is no attempt at one:

- The bundle is an ES module the SPA `import()`s into the page. It shares the
  document, the Svelte runtime, the DOM and the same-origin cookie with cctui.
  Nothing stops it reading or calling anything the page can.
- `apiFetch` is therefore a convenience, not a boundary: the plugin could call
  `fetch('/api/v1/...')` itself and the cookie would ride along either way. It
  acts with the authority of whoever is looking at it — no more (the server still
  enforces that user's scopes) and no less.
- The gates are social, not technical: **an admin** installs the archive and turns
  it on for the instance, and **each user** switches it on for themselves. Install
  a plugin you would let commit to this repo, from a pinned catalog entry whose
  sha256 is checked, and nothing else.
- `pluginFetch` exists so a plugin's backend needs no browser-held token: the
  server signs the caller's identity upstream. That protects the *upstream
  secret*, not the browser — a plugin can still call its own backend as the user.
- Secrets in `instanceSettings` are write-only and never returned by the API, so
  a plugin's frontend cannot read them even though its backend can be reached
  through the proxy.

## Endpoints

| Route | Auth | Purpose |
| --- | --- | --- |
| `GET /api/v1/plugins` | bearer, read | `PluginInfo[]`: manifest fields, `web` as `/plugins/<id>/<web>?v=<sha8>`, `page`, `styles` (resolved the same way), `enabled`, `settings` declarations and the caller's `config` values |
| `ANY /api/v1/plugins/{id}/backend/{*path}` | bearer or cookie, read | proxy to the plugin's backend with signed identity headers; streams, incl. SSE |
| `POST /api/v1/plugins/rescan` | admin | re-read the plugins directory |
| `GET /api/v1/admin/plugins` | admin | `AdminPluginInfo[]`: every plugin, installed or from the directory, with its instance toggle |
| `GET /api/v1/admin/plugins/catalog` | admin | `CatalogPluginInfo[]`: the published catalog, annotated with `installed_version` and `update_available` |
| `POST /api/v1/admin/plugins` | admin | install or upgrade from `{catalog}`, `{url}` or a multipart `file` |
| `PATCH /api/v1/admin/plugins/{id}` | admin | `{enabled}`, the instance-wide toggle |
| `GET /api/v1/admin/plugins/{id}/settings` | admin | `PluginInstanceSettings`: declarations, non-secret values, which secrets are set |
| `PUT /api/v1/admin/plugins/{id}/settings` | admin | patch `{values}`; `""` clears a key |
| `POST /api/v1/admin/plugins/{id}/proxy-secret` | admin | rotate the backend-proxy secret, returned once |
| `DELETE /api/v1/admin/plugins/{id}` | admin | uninstall (directory plugins cannot be removed) |
| `GET /plugins/{id}/{path}` | none | static files from the plugin: traversal-safe, typed by extension, `X-Content-Type-Options: nosniff`, `Cache-Control: no-cache` + `ETag` |

The static route is public like the SPA assets because the webui `import()`s
the module, and serves installed plugins out of memory and directory plugins off
disk. Routing in front of the server must send `/plugins/*` to it alongside
`/api/*`.

## Per-user state

Everything user-facing lives in the user's settings blob:

```json
{ "plugins": { "enabled": { "yubisashi": true }, "config": { "yubisashi": { "host": "10.0.0.5" } } } }
```

`PUT /settings` keeps only installed plugin ids, only declared keys, and
string values of at most 512 characters.

## Skills and env in agent sessions

When a session (re)launches, the daemon's gateway-env pull returns, for every
plugin the session owner enabled, the skill file list, a content hash, and the
resolved `env` (declared name → the owner's value, empty values omitted).

The daemon mirrors the skill files from `GET /plugins/<id>/skills/<file>` into
`$XDG_CONFIG_HOME/cctui/plugins/<id>/<hash>/` (override with
`CCTUI_PLUGIN_CACHE_DIR`) as a Claude Code plugin (`.claude-plugin/plugin.json`
+ `skills/`). A mirror is fetched once per hash; a plugin that fails to mirror
is skipped, never blocking the launch.

Each harness is then handed the mirror through the only channel it has for
per-session skills:

- **Claude Code**: one `--plugin-dir` per plugin. The flag rides the respawn
  flags too, so `/clear`, `/compact` and CLI upgrades keep the skills.
- **Codex**: one shared `codex app-server` serves every session on the machine,
  so nothing process-global may be used (`skills/extraRoots/set` would leak one
  user's skills to all of them). Instead each `thread/{start,resume,fork}`
  carries a `<cctui_skills>` catalog — name, description and `SKILL.md` path per
  skill — as `developerInstructions`, and the thread's shell env as
  `config.shell_environment_policy.set`. Codex persists neither, so both ride
  every resume and fork.
- **opencode**: `opencode serve` runs per session, so the mirrors are listed as
  `skills.paths` in the session's generated `opencode.json` and the env goes on
  the serve process.

The env values are exported into the session's environment after the gateway
env, never overriding a key already set, and re-validated against the same
name rules. Every agent session also gets `CCTUI_WEB_ORIGIN` (the scheme and
authority of the server the daemon talks to) so a skill can address the webui
the user is on.

## Previews (dev servers in a pane)

A plugin pane can frame a dev server running next to the agent — on any machine,
including a k8s worker — without exposing a port.

Set `CCTUI_PREVIEW_HOST` on the server to a pattern containing `{id}`, e.g.
`cctui-pv-{id}.dorsk.dev`. Unset = feature off: `preview open` fails with
`previews are disabled on this instance` and the session API answers
`503` with that message, so a pane says so instead of waiting for a preview
that can never appear. Routing in front of the server must send `*.dorsk.dev`
to it (exact hostnames still win), and the wildcard TLS cert must cover it.

The pattern is a bare host, without scheme or port; the port comes from
`CCTUI_EXTERNAL_URL`, so a server reached at `http://localhost:8700` hands out
`http://cctui-pv-<id>.localhost:8700`.

### Previews on localhost (self-hosted, no DNS, no TLS)

`CCTUI_PREVIEW_HOST=cctui-pv-{id}.localhost` is all a local instance needs:
browsers resolve every `*.localhost` name to loopback themselves, so there is
no wildcard DNS record and no certificate to issue, and the preview is served
by the same server on the same port. The local stack (`deploy/local`) sets it
by default. A browser also needs the preview origin in the page's
`frame-src` to frame it — `deploy/local/nginx.conf` allows
`http://*.localhost:*`.

Inside a session, `cctui-daemon preview open --port <n>` registers the port and
prints the preview URL; `preview close --port <n>` drops it. Previews also close
when the session ends or that daemon disconnects. The session id comes from
`--session` or `CCTUI_SESSION_ID`, which Claude Code, Codex and opencode
sessions all get. Its value is the id the server keyed the session on at launch,
which for Codex and opencode is not the thread/session id the harness later
mints; the daemon aliases one onto the other, so both resolve.

Requests whose `Host` matches the pattern are handled before the regular router:
the server looks up the preview id and tunnels the request down that daemon's
existing WS as streamed request/response frames, WebSocket upgrades included, so
HMR works. Unknown ids 404; every other Host keeps today's behaviour.

### Multiple replicas

Previews are shared state, because a daemon's WS lands on one replica while the
browser can reach any of them. The `previews` table (migration 142) is the source
of truth for id → session/owner/machine/port, so every pod resolves every
preview; only the tunnel's stream state is pod-local.

A pod that receives a preview request it cannot serve itself looks up the pod
holding that daemon's WS in `ws_presence` and reverse-proxies the raw request to
its `/internal/preview/{id}/{*path}`, streaming bodies both ways and bridging
WebSocket upgrades. That endpoint takes the same cluster-internal secret as
`/internal/bus/*` and serves **locally only** — it never forwards again, so a
stale presence row cannot start a loop. Ticket nonces are burnt in
`preview_tickets_used`, so a ticket minted on one pod is single-use across all
of them.

Rolling restarts keep previews alive. A daemon that loses its WS keeps its open
previews and re-announces them by id when it reconnects (possibly to a different
pod); the server re-binds an existing row only when machine, session, user and
port all match. Meanwhile the row is marked detached rather than deleted, and a
sweeper drops it once it has been detached for more than
`DETACH_GRACE_SECS` (2 min). Session end still deletes immediately.

The daemon snapshots its open previews to `~/.config/cctui/previews.json`, so
a self-update re-exec (a server release usually triggers one within minutes of
the rollout) still re-announces them on its first connect. A re-announce the
server refuses is dropped from the snapshot. Both sides log the exchange
(`re-announcing preview` / `preview re-announce accepted|refused` on the
daemon, `preview re-bound to this pod` / `preview re-announce matched no row`
on the server).

The cross-pod hop forwards the browser's request headers, including the app's
own cookies; only cctui's `cctui_auth` and `cctui_preview` cookies are stripped
before the hop, exactly as on the tunnel itself.

This needs no extra configuration beyond what the peer mesh already requires
(`CCTUI_POD_IP`); without it a single replica keeps working unchanged.

### Security model

- **Owner only.** `GET /api/v1/sessions/{id}/previews` lists a session's
  previews and `POST …/previews/{pid}/ticket` mints a 60 s single-use signed
  ticket; both refuse anyone but the session owner.
- **Cookie gate.** `GET https://<preview host>/__cctui/auth?ticket=…` redeems the
  ticket and sets a host-only `cctui_preview` cookie (HttpOnly, Secure,
  SameSite=Lax, no Domain) bound to that preview id and user, then redirects to
  `/`. Every other preview request needs it, else 401.
- **Nothing leaks either way.** cctui's own auth cookie and headers are stripped
  before a request is forwarded, hop-by-hop headers are dropped, and the daemon
  only ever connects to `127.0.0.1:<registered port>`. `Host` and, on WebSocket
  upgrades, `Origin` are rewritten to `localhost:<port>` — Vite rejects HMR
  upgrades whose Origin is not its own host.
- **CSRF.** Preview hosts are same-site with cctui, so cookie-authenticated
  requests with an unsafe method (POST/PUT/PATCH/DELETE) are rejected unless
  their `Origin` (else `Referer`) is an allowed origin. Bearer-authenticated
  requests are unaffected.
