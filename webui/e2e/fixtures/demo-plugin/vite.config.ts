import { defineConfig } from 'vite';
import { cctuiPluginConfig } from '../../../plugin-sdk/vite';

export default defineConfig(cctuiPluginConfig({ entry: 'src/index.ts', outDir: process.env.DEMO_PLUGIN_OUT ?? 'dist/web' }));
