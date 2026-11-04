import { describe, expect, it } from 'vitest';
import { HOLD_MS, MIN_CLIP_MS, TapHold } from './tapHold';

describe('TapHold', () => {
	it('a tap starts recording and keeps it going after release', () => {
		const g = new TapHold();
		expect(g.down(0)).toBe('start');
		expect(g.up(100)).toBe('none');
		expect(g.recording).toBe(true);
	});

	it('a second tap stops', () => {
		const g = new TapHold();
		g.down(0);
		g.up(100);
		expect(g.down(5000)).toBe('stop');
		expect(g.recording).toBe(false);
		expect(g.up(5080)).toBe('none');
	});

	it('a press held past the hold time stops on release', () => {
		const g = new TapHold();
		expect(g.down(0)).toBe('start');
		expect(g.up(HOLD_MS + 50)).toBe('stop');
		expect(g.recording).toBe(false);
	});

	it('a quick double tap cancels instead of sending an empty clip', () => {
		const g = new TapHold();
		g.down(0);
		g.up(60);
		expect(g.down(MIN_CLIP_MS - 100)).toBe('cancel');
		expect(g.recording).toBe(false);
	});

	it('the pointer leaving mid-press counts as a release', () => {
		const g = new TapHold();
		g.down(0);
		expect(g.up(50)).toBe('none');
		expect(g.recording).toBe(true);
		expect(g.up(2000)).toBe('none');
		expect(g.recording).toBe(true);
	});

	it('toggle starts and stops for the keyboard and the stop control', () => {
		const g = new TapHold();
		expect(g.toggle(0)).toBe('start');
		expect(g.toggle(100)).toBe('cancel');
		expect(g.toggle(200)).toBe('start');
		expect(g.toggle(2000)).toBe('stop');
	});

	it('a release of a press that did not start the recording does nothing', () => {
		const g = new TapHold();
		expect(g.up(10)).toBe('none');
		g.down(0);
		g.up(50);
		g.down(1000);
		expect(g.recording).toBe(false);
		expect(g.up(1400)).toBe('none');
		expect(g.recording).toBe(false);
	});

	it('a tap that lingers short of the hold time still records hands-free', () => {
		const g = new TapHold();
		expect(g.down(0)).toBe('start');
		expect(g.up(HOLD_MS - 1)).toBe('none');
		expect(g.recording).toBe(true);
	});

	it('a long hands-free dictation is stopped and sent by the next tap', () => {
		const g = new TapHold();
		g.down(0);
		g.up(120);
		expect(g.recording).toBe(true);
		expect(g.down(30_000)).toBe('stop');
		expect(g.up(30_090)).toBe('none');
		expect(g.recording).toBe(false);
	});
});
