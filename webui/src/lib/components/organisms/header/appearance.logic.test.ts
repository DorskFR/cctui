import { describe, expect, it } from 'vitest';
import { scaleLabel } from './appearance.logic';

describe('scaleLabel', () => {
	const levels = [
		{ id: 'sm', label: 'Small' },
		{ id: 'md', label: 'Default' }
	];
	it('names the current text size', () => {
		expect(scaleLabel(levels, 'md')).toBe('Default');
	});
	it('falls back to the raw id for an unknown level', () => {
		expect(scaleLabel(levels, 'xl')).toBe('xl');
	});
});
