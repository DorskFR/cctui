import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const header = readFileSync(
	'src/lib/components/organisms/conversation/DrawerHeader.svelte',
	'utf8'
);
const registry = readFileSync(
	'node_modules/@dorsk/tsumikit/dist/components/atoms/Icon.svelte',
	'utf8'
);

const backButton = () => {
	const i = header.indexOf('label={m.drawer_back()}');
	expect(i).toBeGreaterThan(-1);
	const start = header.lastIndexOf('<IconButton', i);
	return header.slice(start, header.indexOf('/>', i) + 2);
};

const glyph = (name: string) => {
	const m = registry.match(new RegExp(`(?:'${name}'|${name}):\\s*'([^']*)'`));
	expect(m, `no "${name}" glyph in the kit registry`).toBeTruthy();
	return (m as RegExpMatchArray)[1];
};

describe('drawer back control', () => {
	it('uses the bare chevron, not the arrow', () => {
		expect(backButton()).toContain('icon="chevron-left"');
		expect(backButton()).not.toContain('icon="back"');
	});

	it('stays a quiet affordance — no chip box, no oversized glyph', () => {
		const btn = backButton();
		expect(btn).not.toContain('glyphSize');
		expect(btn).not.toContain('chip');
	});

	it('the kit glyph it points at is a chevron with no arrow shaft', () => {
		expect(glyph('back')).toContain('M19 12H5');
		expect(glyph('chevron-left')).not.toContain('M19 12H5');
		expect(glyph('chevron-left')).toBe('<path d="m15 18-6-6 6-6" />');
	});
});
