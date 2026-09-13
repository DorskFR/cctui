import { describe, expect, it } from 'vitest';
import type { Interaction } from '@dorsk/journey';
import type { ShowCtx } from '@dorsk/journey/runtime';
import { actionHint } from './guideHint';

const ctx = (kind: Interaction['kind'], next: (() => void) | null = null) =>
	({ action: { kind } as Interaction, next }) as Pick<ShowCtx, 'action' | 'next'>;

describe('actionHint', () => {
	it('names the interaction a wait-for-user step is waiting for', () => {
		expect(actionHint(ctx('click'))).toBeTruthy();
		expect(actionHint(ctx('dblclick'))).toBeTruthy();
		expect(actionHint(ctx('fill'))).toBeTruthy();
		expect(actionHint(ctx('select'))).toBeTruthy();
		expect(actionHint(ctx('check'))).toBeTruthy();
		expect(actionHint(ctx('hover'))).toBeTruthy();
	});

	it('stays quiet when a Next button already says how to carry on', () => {
		expect(actionHint(ctx('click', () => {}))).toBeUndefined();
		expect(actionHint(ctx('none', () => {}))).toBeUndefined();
	});

	it('leaves press to the runtime toast, and none with nothing to say', () => {
		expect(actionHint(ctx('press'))).toBeUndefined();
		expect(actionHint(ctx('none'))).toBeUndefined();
	});
});
