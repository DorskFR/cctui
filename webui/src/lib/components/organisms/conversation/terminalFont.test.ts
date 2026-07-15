import { describe, it, expect, afterEach } from 'vitest';
import { normalizeFontStack, resolveTerminalFont, BUNDLED_TERMINAL_FONT } from './terminalFont';

describe('normalizeFontStack', () => {
	it('replaces an unresolved var() with a concrete stack, never returns var()', () => {
		const out = normalizeFontStack('var(--font-mono, monospace)');
		expect(out).not.toContain('var(');
		expect(out).toContain('monospace');
	});

	it('prepends the bundled font to a concrete stack', () => {
		expect(normalizeFontStack('Menlo, Consolas, monospace')).toBe(
			`"${BUNDLED_TERMINAL_FONT}", Menlo, Consolas, monospace`
		);
	});

	it('does not duplicate the bundled font when already present', () => {
		const stack = '"JetBrains Mono", monospace';
		expect(normalizeFontStack(stack)).toBe(stack);
	});

	it('falls back to a concrete monospace stack when empty', () => {
		const out = normalizeFontStack('   ');
		expect(out).toContain(BUNDLED_TERMINAL_FONT);
		expect(out).toContain('monospace');
		expect(out).not.toContain('var(');
	});
});

describe('resolveTerminalFont', () => {
	afterEach(() => {
		document.documentElement.style.removeProperty('--font-mono');
	});

	it('reads --font-mono from the document and returns a concrete stack', () => {
		document.documentElement.style.setProperty('--font-mono', 'Menlo, monospace');
		const out = resolveTerminalFont(document);
		expect(out).toBe(`"${BUNDLED_TERMINAL_FONT}", Menlo, monospace`);
		expect(out).not.toContain('var(');
	});
});
