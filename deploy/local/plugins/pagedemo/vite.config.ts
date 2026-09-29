import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import { cctuiPluginConfig } from '../../../../webui/plugin-sdk/vite';

// Built from webui (`npm run plugins:local`), so every path is absolute.
const here = dirname(fileURLToPath(import.meta.url));

export default defineConfig({
	root: here,
	...cctuiPluginConfig({ entry: join(here, 'src/index.ts'), outDir: join(here, 'web') })
});
