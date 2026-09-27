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
optional `icon` (a Tsumikit icon name), `web`, `skills` and `settings`
(`[{ key, label, env, type: "string" }]`, rendered as a form in
Settings → Plugins; values reach the user's sessions as the declared env var).

## Module contract

`web/index.js` is an ES module whose default export is a `CctuiPluginModule`:

```ts
import type { CctuiPluginModule } from '@dorsk/cctui-plugin-sdk/types';
import Pane from './Pane.svelte';

export default {
	cctuiApi: 1,
	sessionPane: Pane,
	messageActions: (msg) =>
		/^yubisashi: (https?:\S+)/m.test(msg.text)
			? [{ label: 'Open in Review', icon: 'eye', params: { url: RegExp.$1 }, open: 'sessionPane', autoOpen: true }]
			: []
} satisfies CctuiPluginModule;
```

- `sessionPane` is mounted with `PaneProps` (`session`, `composer`, `params`, `onclose`)
  in a resizable column beside the conversation drawer.
- `messageActions` runs for every assistant line; each returned action is a
  button on that line that opens the pane with `params`. `autoOpen: true` asks
  the host to open the pane itself, once per (session, params), for the newest line.
- The host sets a Svelte context under `HOST_CONTEXT_KEY` (`{ cctuiApi, origin }`).

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
