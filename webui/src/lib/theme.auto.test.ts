import { afterEach, describe, expect, it, vi } from 'vitest';

// The "Auto" theme defers to `prefers-color-scheme`. The Theme singleton reads
// localStorage and the media query in its constructor, so every case re-imports
// the module against a freshly stubbed environment.

type Listener = (e: { matches: boolean }) => void;

function stubMatchMedia(dark: boolean) {
	const listeners: Listener[] = [];
	vi.stubGlobal('matchMedia', (query: string) => ({
		matches: query.includes('dark') ? dark : false,
		media: query,
		addEventListener: (_: string, fn: Listener) => listeners.push(fn),
		removeEventListener: () => {}
	}));
	// Emulate the OS flipping while the page is open.
	return (matches: boolean) => {
		for (const fn of listeners) fn({ matches });
	};
}

async function load(saved: string | null, dark: boolean) {
	const flip = stubMatchMedia(dark);
	vi.resetModules();
	localStorage.clear();
	if (saved !== null) localStorage.setItem('cctui_theme', saved);
	const mod = await import('./theme.svelte');
	return { ...mod, flip };
}

function painted(): string | null {
	return document.documentElement.getAttribute('data-theme');
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe('auto theme', () => {
	it('resolves to dark when the browser prefers dark', async () => {
		const { theme } = await load('auto', true);
		expect(theme.current).toBe('auto');
		expect(theme.isAuto).toBe(true);
		expect(theme.resolved).toBe('dark');
		expect(painted()).toBe('dark');
	});

	it('resolves to light when the browser prefers light', async () => {
		const { theme } = await load('auto', false);
		expect(theme.resolved).toBe('light');
		expect(painted()).toBe('light');
	});

	it('never paints the literal "auto" onto data-theme', async () => {
		await load('auto', true);
		expect(painted()).not.toBe('auto');
	});

	it('repaints when the OS preference flips mid-session', async () => {
		const { theme, flip } = await load('auto', false);
		expect(theme.resolved).toBe('light');
		flip(true);
		expect(theme.resolved).toBe('dark');
		expect(painted()).toBe('dark');
	});

	it('leaves an explicit palette alone when the OS preference flips', async () => {
		const { theme, flip } = await load('gruvbox', false);
		expect(theme.resolved).toBe('gruvbox');
		flip(true);
		expect(theme.resolved).toBe('gruvbox');
		expect(painted()).toBe('gruvbox');
	});

	it('reports the selection, not the resolution, as label and icon', async () => {
		const { theme, AUTO } = await load('auto', true);
		expect(theme.label).toBe(AUTO.label);
		expect(theme.icon).toBe(AUTO.icon);
	});

	it('persists "auto" and accepts it back on reload', async () => {
		const { theme } = await load('dark', true);
		theme.set('auto');
		expect(localStorage.getItem('cctui_theme')).toBe('auto');
		const reloaded = await load('auto', true);
		expect(reloaded.theme.current).toBe('auto');
	});

	it('falls back to a real theme when a stored value is unknown', async () => {
		const { theme } = await load('not-a-theme', true);
		expect(theme.current).toBe('dark');
	});
});
