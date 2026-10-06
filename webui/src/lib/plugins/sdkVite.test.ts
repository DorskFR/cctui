import { describe, expect, it } from 'vitest';
import { cctuiPluginConfig } from '../../../plugin-sdk/vite';

describe('cctuiPluginConfig', () => {
	it('replaces process.env.NODE_ENV, which does not exist in the browser', () => {
		const config = cctuiPluginConfig({ entry: 'src/index.ts' });
		expect(config.define?.['process.env.NODE_ENV']).toBe('"production"');
	});
});
