import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { UserConfig } from 'vite';
import { cctuiPluginConfig } from '../../../../webui/plugin-sdk/vite';

// Built from webui (`npm run plugins:local`), so every path is absolute. A
// runtime import of vite would resolve from here, where no node_modules exist.
const here = dirname(fileURLToPath(import.meta.url));

export default {
	root: here,
	...cctuiPluginConfig({ entry: join(here, 'src/index.ts'), outDir: join(here, 'web') })
} satisfies UserConfig;
