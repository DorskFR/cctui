import { describe, expect, it } from 'vitest';
import { DOCK_MIN_PX } from './dock';
import { draggedWidth, maxDockWidth } from './dockResize';

describe('draggedWidth', () => {
	it('grows a left-docked panel when the pointer moves right, and a right-docked one when it moves left', () => {
		expect(draggedWidth('left', 400, 50, 2000)).toBe(450);
		expect(draggedWidth('right', 400, 50, 2000)).toBe(350);
		expect(draggedWidth('right', 400, -50, 2000)).toBe(450);
	});

	it('never shrinks below the floor nor past the viewport share', () => {
		expect(draggedWidth('left', 400, -1000, 2000)).toBe(DOCK_MIN_PX);
		expect(draggedWidth('left', 400, 5000, 2000)).toBe(maxDockWidth(2000));
		expect(maxDockWidth(2000)).toBe(1200);
	});

	it('keeps the floor on a viewport too narrow for the share', () => {
		expect(maxDockWidth(100)).toBe(DOCK_MIN_PX);
	});
});
