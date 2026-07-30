import { describe, it, expect, afterEach } from 'vitest';
import {
	normalizeFontStack,
	resolveTerminalFont,
	resolveTerminalBg,
	BUNDLED_TERMINAL_FONT,
	FALLBACK_TERMINAL_BG
} from './terminalFont';

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

describe('resolveTerminalBg', () => {
	afterEach(() => {
		document.documentElement.style.removeProperty('--term-bg');
	});

	it('reads --term-bg from the document', () => {
		document.documentElement.style.setProperty('--term-bg', '#123456');
		expect(resolveTerminalBg(document)).toBe('#123456');
	});

	it('falls back to a concrete color when the token is unset', () => {
		expect(resolveTerminalBg(document)).toBe(FALLBACK_TERMINAL_BG);
	});

	it('never returns an unresolved var()', () => {
		document.documentElement.style.setProperty('--term-bg', 'var(--nope)');
		expect(resolveTerminalBg(document)).toBe(FALLBACK_TERMINAL_BG);
	});
});
