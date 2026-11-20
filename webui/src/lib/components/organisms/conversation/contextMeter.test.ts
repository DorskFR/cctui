import { describe, expect, it } from 'vitest';
import { contextMeter, contextTokens, followupDue, type ContextFields } from './contextMeter';

const ctx = (
	context_tokens: number | null,
	context_window: number | null,
	auto_followup_at: number | null = null
): ContextFields => ({ context_tokens, context_window, auto_followup_at });

describe('contextMeter', () => {
	it('formats tokens and window as 182k / 1M · 18%', () => {
		const meter = contextMeter(ctx(182_000, 1_000_000));
		expect(meter?.label).toBe('182k / 1M · 18%');
		expect(meter?.fill).toBeCloseTo(0.182);
		expect(meter?.mark).toBeNull();
		expect(contextMeter(ctx(160_000, 200_000))?.label).toBe('160k / 200k · 80%');
		expect(contextTokens(1_500_000)).toBe('1.5M');
	});

	it('without a window shows tokens only', () => {
		const meter = contextMeter(ctx(182_000, null, 150_000));
		expect(meter?.label).toBe('182k');
		expect(meter?.fill).toBeNull();
		expect(meter?.mark).toBeNull();
		expect(meter?.followupAt).toBe(150_000);
		expect(contextMeter(ctx(null, 1_000_000))).toBeNull();
	});

	it('marks the threshold when auto follow-up is on', () => {
		const meter = contextMeter(ctx(160_000, 200_000, 150_000));
		expect(meter?.mark).toBeCloseTo(0.75);
		expect(meter?.followupAt).toBe(150_000);
		expect(contextMeter(ctx(160_000, 200_000, 400_000))?.mark).toBe(1);
		expect(contextMeter(ctx(160_000, 200_000, null))?.mark).toBeNull();
	});

	it('the next send follows up at or above the threshold', () => {
		expect(followupDue(ctx(150_000, 200_000, 150_000))).toBe(true);
		expect(followupDue(ctx(160_000, null, 150_000))).toBe(true);
		expect(followupDue(ctx(149_999, 200_000, 150_000))).toBe(false);
		expect(followupDue(ctx(160_000, 200_000, null))).toBe(false);
		expect(followupDue(ctx(null, 200_000, 150_000))).toBe(false);
	});
});
