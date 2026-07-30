export const BUNDLED_TERMINAL_FONT = 'JetBrains Mono';

const FALLBACK_STACK = 'ui-monospace, "SF Mono", "Fira Code", Menlo, Consolas, monospace';

function quoteIfNeeded(family: string): string {
	return /\s/.test(family) && !/^["']/.test(family) ? `"${family}"` : family;
}

// xterm measures glyph advance width via a canvas 2D `font`, which cannot
// resolve CSS var(); it must receive a concrete stack, never a var(...) string.
export function normalizeFontStack(raw: string): string {
	let stack = raw.trim();
	if (!stack || stack.includes('var(')) {
		stack = FALLBACK_STACK;
	}
	const hasBundled = stack.toLowerCase().includes(BUNDLED_TERMINAL_FONT.toLowerCase());
	return hasBundled ? stack : `${quoteIfNeeded(BUNDLED_TERMINAL_FONT)}, ${stack}`;
}

export function resolveTerminalFont(doc: Document = document): string {
	let raw = '';
	try {
		raw = getComputedStyle(doc.documentElement).getPropertyValue('--font-mono');
	} catch {
		raw = '';
	}
	return normalizeFontStack(raw);
}

export const FALLBACK_TERMINAL_BG = '#0b0e14';

export function resolveTerminalBg(doc: Document = document): string {
	let raw = '';
	try {
		raw = getComputedStyle(doc.documentElement).getPropertyValue('--term-bg');
	} catch {
		raw = '';
	}
	const bg = raw.trim();
	return !bg || bg.includes('var(') ? FALLBACK_TERMINAL_BG : bg;
}
