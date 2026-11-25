import { describe, expect, it } from 'vitest';
import lead from './Lead.svelte?raw';

describe('card title', () => {
	it('paints in the default text tone instead of inheriting a muted ancestor', () => {
		const title = lead.slice(lead.indexOf('<Text\n\t\tdata-journey="title"'));
		expect(title.slice(0, title.indexOf('>'))).toContain('tone="default"');
	});
});
