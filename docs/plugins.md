# Runtime plugins

A plugin is a bundle an admin installs into the instance. It can contribute a
webui pane (an ES module the SPA imports at runtime), Claude Code skills for
the user's agent sessions, and per-user settings that reach those sessions as
environment variables. Nothing is baked into the images and nothing is written
into the user's repository.

## Install and enable

Two gates, both required before a user sees anything:

1. **Instance** — an admin installs the plugin and turns it on in
   Settings → Instance → Plugins. `GET /api/v1/plugins` only ever returns
   instance-enabled plugins.
2. **User** — each user flips their own switch in Settings → Plugins. The
   per-plugin settings form only renders once that switch is on.

Installed plugins live in the `plugins` table: the manifest plus the archive in
the existing blob store, extracted into memory at install and at startup. No
writable volume is needed, so a plugin survives a pod restart and reaches every
replica.

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
- `settings` (optional): each entry declares a per-user string value. `env`
  must match `^[A-Z][A-Z0-9_]{0,63}$` and may not be a reserved name
  (`PATH`, `HOME`, `SHELL`, `USER`, `NODE_OPTIONS`, `LD_*`, `DYLD_*`,
  `ANTHROPIC_*`, `CLAUDE_*`, `CCTUI_*`, `OPENAI_*`, `FIREWORKS_*`, `*_PROXY`,
  and the daemon's own contract vars).

## Endpoints

| Route | Auth | Purpose |
| --- | --- | --- |
| `GET /api/v1/plugins` | bearer, read | `PluginInfo[]`: manifest fields, `web` as `/plugins/<id>/<web>?v=<sha8>`, `enabled`, `settings` declarations and the caller's `config` values |
| `POST /api/v1/plugins/rescan` | admin | re-read the plugins directory |
| `GET /api/v1/admin/plugins` | admin | `AdminPluginInfo[]`: every plugin, installed or from the directory, with its instance toggle |
| `POST /api/v1/admin/plugins` | admin | install or upgrade from `{url}` or a multipart `file` |
| `PATCH /api/v1/admin/plugins/{id}` | admin | `{enabled}`, the instance-wide toggle |
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
+ `skills/`) and launches the worker with one `--plugin-dir` per plugin. The
flag rides the respawn flags too, so `/clear`, `/compact` and CLI upgrades
keep the skills. A mirror is fetched once per hash; a plugin that fails to
mirror is skipped, never blocking the launch.

The env values are exported into the worker's environment after the gateway
env, never overriding a key already set, and re-validated against the same
name rules. Every agent session also gets `CCTUI_WEB_ORIGIN` (the scheme and
authority of the server the daemon talks to) so a skill can address the webui
the user is on.

## Previews (dev servers in a pane)

A plugin pane can frame a dev server running next to the agent — on any machine,
including a k8s worker — without exposing a port.

Set `CCTUI_PREVIEW_HOST` on the server to a pattern containing `{id}`, e.g.
`cctui-pv-{id}.dorsk.dev`. Unset = feature off, and every preview route 404s.
Routing in front of the server must send `*.dorsk.dev` to it (exact hostnames
still win), and the wildcard TLS cert must cover it.

Inside a session, `cctui-daemon preview open --port <n>` registers the port and
prints the preview URL; `preview close --port <n>` drops it. Previews also close
when the session ends or that daemon disconnects. The session id comes from
`CCTUI_SESSION_ID`, which every agent session now gets.

Requests whose `Host` matches the pattern are handled before the regular router:
the server looks up the preview id and tunnels the request down that daemon's
existing WS as streamed request/response frames, WebSocket upgrades included, so
HMR works. Unknown ids 404; every other Host keeps today's behaviour.

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
