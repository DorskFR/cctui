import { describe, expect, it } from 'vitest';
import { nextRadioIndex } from './radioNav';

describe('nextRadioIndex', () => {
	it('moves back on ArrowUp / ArrowLeft and forward on ArrowDown / ArrowRight', () => {
		expect(nextRadioIndex('ArrowUp', 1, 3)).toBe(0);
		expect(nextRadioIndex('ArrowLeft', 1, 3)).toBe(0);
		expect(nextRadioIndex('ArrowDown', 1, 3)).toBe(2);
		expect(nextRadioIndex('ArrowRight', 1, 3)).toBe(2);
	});

	it('wraps at both ends', () => {
		expect(nextRadioIndex('ArrowUp', 0, 3)).toBe(2);
		expect(nextRadioIndex('ArrowDown', 2, 3)).toBe(0);
	});

	it('jumps to the ends on Home / End', () => {
		expect(nextRadioIndex('Home', 2, 3)).toBe(0);
		expect(nextRadioIndex('End', 0, 3)).toBe(2);
	});

	it('stays on the only row of a single-row group', () => {
		expect(nextRadioIndex('ArrowDown', 0, 1)).toBe(0);
		expect(nextRadioIndex('ArrowUp', 0, 1)).toBe(0);
	});

	it('leaves non-navigating keys and out-of-range state unhandled', () => {
		expect(nextRadioIndex('Tab', 0, 3)).toBeNull();
		expect(nextRadioIndex(' ', 0, 3)).toBeNull();
		expect(nextRadioIndex('Enter', 0, 3)).toBeNull();
		expect(nextRadioIndex('ArrowDown', 0, 0)).toBeNull();
		expect(nextRadioIndex('ArrowDown', -1, 3)).toBeNull();
		expect(nextRadioIndex('ArrowDown', 3, 3)).toBeNull();
	});
});
