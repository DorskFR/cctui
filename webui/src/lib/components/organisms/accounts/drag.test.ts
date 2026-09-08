import { describe, expect, it } from 'vitest';
import { isTouchPointer } from './drag.svelte';

// happy-dom implements no HTML5 drag and drop, so the mouse drag itself can only
// be exercised in a real browser; this covers the rule that keeps the two paths
// apart.
describe('isTouchPointer', () => {
	it('claims touch and pen', () => {
		expect(isTouchPointer({ pointerType: 'touch' })).toBe(true);
		expect(isTouchPointer({ pointerType: 'pen' })).toBe(true);
	});

	it('leaves the mouse to HTML5 drag and drop', () => {
		expect(isTouchPointer({ pointerType: 'mouse' })).toBe(false);
	});
});
