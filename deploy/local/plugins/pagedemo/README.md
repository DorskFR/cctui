# pagedemo — page-surface fixture

A directory plugin for the local stack whose only contribution is a `page`: it
routes `/`, `/item/<name>` under `/apps/pagedemo` and calls the host's
`navigate`, so back/forward and a deep link are exercised end to end.

`web/index.js` is a build artifact and is not committed. Build it from `webui`
(its `node_modules` has vite, svelte and the SDK):

```sh
cd webui && npm run plugins:local
```

`make local/up` then serves it: `deploy/local/plugins` is mounted read-only as
`CCTUI_PLUGINS_DIR`, directory plugins are always instance-enabled, and each
user still switches it on in Settings › Plugins.

The `apps-page-plugin` journey (`webui/journeys/`) drives it; its steps are
`qaOnly`, so the public tour never mentions a fixture.
