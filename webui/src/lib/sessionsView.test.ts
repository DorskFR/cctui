// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { LIST_VIEW } from './drafts';
import {
	isViewMode,
	parseViewMode,
	serializeViewMode,
	sessionsView,
	type ViewMode
} from './sessionsView.svelte';

afterEach(() => {
	sessionsView.mode = 'list';
	sessionsView.wide = true;
});

describe('view mode parsing', () => {
	it('accepts all three modes, in either direction', () => {
		for (const v of ['list', 'grid', 'tiles'] as ViewMode[]) {
			expect(parseViewMode(v), v).toBe(v);
		}
		expect(serializeViewMode('list')).toBe('list');
		expect(serializeViewMode('tiles')).toBe('tiles');
	});

	it('reads the legacy `card` as grid and keeps writing it', () => {
		expect(parseViewMode('card')).toBe('grid');
		expect(serializeViewMode('grid')).toBe('card');
	});

	it('falls back to the list for anything it does not know', () => {
		for (const bad of ['', null, undefined, 'TILES', 'mosaic', '../../etc']) {
			expect(parseViewMode(bad), String(bad)).toBe('list');
		}
	});

	it('only recognises the three canonical names, so a bad ?view is ignored', () => {
		expect(isViewMode('tiles')).toBe(true);
		expect(isViewMode('card')).toBe(false);
		expect(isViewMode('nope')).toBe(false);
	});
});

describe('the view mode is its own persistence', () => {
	it('writes every change straight to storage', () => {
		sessionsView.mode = 'tiles';
		expect(localStorage.getItem(LIST_VIEW)).toBe('tiles');
		sessionsView.mode = 'grid';
		expect(localStorage.getItem(LIST_VIEW)).toBe('card');
	});

	it('keeps a tiles choice stored but unhonoured on a narrow viewport', () => {
		sessionsView.mode = 'tiles';
		sessionsView.wide = false;
		expect(sessionsView.effective).toBe('list');
		expect(sessionsView.tiles).toBe(false);
		expect(sessionsView.mode).toBe('tiles');
		expect(localStorage.getItem(LIST_VIEW)).toBe('tiles');
		sessionsView.wide = true;
		expect(sessionsView.tiles).toBe(true);
	});

	it('leaves the other modes alone on a narrow viewport', () => {
		sessionsView.mode = 'grid';
		sessionsView.wide = false;
		expect(sessionsView.effective).toBe('grid');
	});
});
