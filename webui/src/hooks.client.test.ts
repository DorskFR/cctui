import { describe, expect, it } from 'vitest';
import { isBenignNotice } from './hooks.client';

describe('isBenignNotice', () => {
	it('swallows the ResizeObserver loop notice the browser raises as an error', () => {
		expect(isBenignNotice('ResizeObserver loop completed with undelivered notifications.')).toBe(
			true
		);
		expect(isBenignNotice('ResizeObserver loop limit exceeded')).toBe(true);
		expect(isBenignNotice('  ResizeObserver loop completed  ')).toBe(true);
	});

	it('lets real failures through, including ones that merely mention the API', () => {
		expect(isBenignNotice('TypeError: x is not a function')).toBe(false);
		expect(isBenignNotice('ResizeObserver is not defined')).toBe(false);
		expect(isBenignNotice('')).toBe(false);
	});
});
