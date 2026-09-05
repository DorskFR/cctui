import { describe, expect, it } from 'vitest';
import {
	accountAvatarColors,
	accountHue,
	accountInitial,
	avatarColorsForHue,
	isValidAccountEmoji
} from './avatar';

describe('accountHue', () => {
	it('is stable and in range for the same id', () => {
		const id = '8f14e45f-ceea-467a-9575-4d1a1b2c3d4e';
		expect(accountHue(id)).toBe(accountHue(id));
		expect(accountHue(id)).toBeGreaterThanOrEqual(0);
		expect(accountHue(id)).toBeLessThan(360);
	});

	it('separates different ids', () => {
		const hues = new Set(
			['a', 'b', 'c', 'work', 'personal', '11111111-2222-3333-4444-555555555555'].map(accountHue)
		);
		expect(hues.size).toBeGreaterThan(4);
	});
});

describe('accountAvatarColors', () => {
	it('renders an hsl fill with a contrasting text colour', () => {
		const { background, color } = accountAvatarColors('work');
		expect(background).toMatch(/^hsl\(\d+ 55% 45%\)$/);
		expect(['#14161a', '#ffffff']).toContain(color);
	});

	it('picks dark text on the bright hues and white on the dark ones', () => {
		expect(avatarColorsForHue(60).color).toBe('#14161a');
		expect(avatarColorsForHue(120).color).toBe('#14161a');
		expect(avatarColorsForHue(240).color).toBe('#ffffff');
		expect(avatarColorsForHue(280).color).toBe('#ffffff');
	});
});

describe('accountInitial', () => {
	it('upper-cases the first grapheme', () => {
		expect(accountInitial('work')).toBe('W');
		expect(accountInitial('  éditeur')).toBe('É');
		expect(accountInitial('🐙 octo')).toBe('🐙');
	});

	it('falls back when there is no name', () => {
		expect(accountInitial('')).toBe('?');
		expect(accountInitial('   ')).toBe('?');
	});
});

describe('isValidAccountEmoji', () => {
	it('accepts one emoji grapheme, including sequences and flags', () => {
		for (const ok of ['🐙', '❤️', '👍🏽', '👨‍👩‍👧', '🇫🇷', '☀', '']) {
			expect(isValidAccountEmoji(ok), ok).toBe(true);
		}
	});

	it('rejects text and multiple glyphs', () => {
		for (const bad of ['🐙🐙', 'hi', 'a', '🐙 🐙', '🇫🇷🇫🇷', '🐙‍', '🐙x', '🐙🐙🐙🐙🐙🐙🐙🐙🐙']) {
			expect(isValidAccountEmoji(bad), bad).toBe(false);
		}
	});
});
