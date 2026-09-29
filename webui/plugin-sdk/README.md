# cctui plugin SDK (contract v1)

Types and a Vite config helper for building a cctui runtime plugin's `web/`
bundle. Self-contained: it only depends on `svelte`, `@sveltejs/vite-plugin-svelte`,
`vite` and (types only) `@dorsk/tsumikit`, so it can move to
`@dorsk/cctui-plugin-sdk` unchanged.

## Plugin layout

```
<CCTUI_PLUGINS_DIR>/<id>/plugin.json
<CCTUI_PLUGINS_DIR>/<id>/web/index.js      # built by the helper below
<CCTUI_PLUGINS_DIR>/<id>/skills/<name>/SKILL.md   # optional
```

`plugin.json` declares `id`, `name`, `description`, `version`, `cctuiApi: 1`,
optional `icon` (a Tsumikit icon name), `web`, `page`, `skills` and `settings`
(`[{ key, label, env, type: "string" }]`, rendered as a form in
Settings → Plugins; values reach the user's sessions as the declared env var).

## Module contract

`web/index.js` is an ES module whose default export is a `CctuiPluginModule`:

```ts
import type { CctuiPluginModule } from '@dorsk/cctui-plugin-sdk/types';
import Pane from './Pane.svelte';
import AppPage from './AppPage.svelte';

export default {
	cctuiApi: 1,
	sessionPane: Pane,
	page: AppPage,
	messageActions: (msg) =>
		/^yubisashi: (https?:\S+)/m.test(msg.text)
			? [{ label: 'Open in Review', icon: 'eye', params: { url: RegExp.$1 }, open: 'sessionPane', autoOpen: true }]
			: []
} satisfies CctuiPluginModule;
```

- `sessionPane` is mounted with `PaneProps` (`session`, `composer`, `params`, `onclose`)
  in a resizable column beside the conversation drawer.
- `page` is mounted with `PageProps` (`basePath`, `path`, `navigate`) as a whole
  screen at `/apps/<id>`, once the manifest declares `page: { title, icon }`. It
  gets a nav entry for every user who switched the plugin on. Route on `path`
  (plugin-relative, always rooted at `/`) and call `navigate(path)` instead of
  touching `history`: the host owns the URL, so back/forward and deep links work
  and `path` simply re-renders. `navigate` refuses an absolute URL.
- `messageActions` runs for every assistant line; each returned action is a
  button on that line that opens the pane with `params`. `autoOpen: true` asks
  the host to open the pane itself, once per (session, params), for the newest line.
- The host sets a Svelte context under `HOST_CONTEXT_KEY` above both surfaces.
  `cctuiApi` is the contract major (`1`); `cctuiApiMinor` counts the additive
  extensions the host has, so feature-check anything beyond `{ cctuiApi, origin }`:

  ```ts
  const host = getContext<HostContext | undefined>(HOST_CONTEXT_KEY);
  host?.toast?.('saved', 'ok');
  const res = await host?.pluginFetch?.('/pulls?state=open');
  host?.openSpawn?.({ prompt: `Review ${url}`, working_dir: repo });
  ```

  `user` is `{ id, name, isAdmin }` once the host knows it. `apiFetch(path)` is the
  cctui API under `/api/v1`; `pluginFetch(path)` is your own backend through the
  host proxy, which signs the user's identity upstream so no token is held in the
  browser. `navigate(path)` moves the host SPA; `openSpawn` opens the host's New
  session form pre-filled, for the user to submit.

  A plugin is admin-trusted code running with the user's session: it shares the
  page, the cookie and the Svelte runtime with cctui and is not sandboxed. See
  `docs/plugins.md` § Security model.

## Building

```ts
// vite.config.ts of the plugin
import { defineConfig } from 'vite';
import { cctuiPluginConfig } from '@dorsk/cctui-plugin-sdk/vite';

export default defineConfig(cctuiPluginConfig({ entry: 'src/index.ts', outDir: 'dist/<id>/web' }));
```

The output is a small ES module that imports `svelte`, `svelte/store` and
`@dorsk/tsumikit` from the host's `/plugin-runtime/*.js` shims, so the plugin
shares the page's Svelte runtime (context, reactivity) and Tsumikit styles.
Component CSS is injected at mount. Build the plugin against the Svelte and
Tsumikit versions the host reports at `/plugin-runtime/manifest.json`.
