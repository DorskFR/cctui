// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import type { IR, IRStep, Interaction } from '@dorsk/journey';
import type { Presenter, ShowCtx } from '@dorsk/journey/runtime';
import { actionHint, guided, withFieldEnter } from './guideHint';
import { publicJourneys } from './journey';

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

describe('guided', () => {
	const ir = (steps: Partial<IRStep>[]) => ({ id: 'j', version: 1, route: '/', level: 'smoke', steps }) as unknown as IR;

	it('offers Next on every step and turns fills into pointing', () => {
		const out = guided(
			ir([
				{ id: 'a', do: { kind: 'click' }, guide: 'wait-for-user' },
				{ id: 'b', do: { kind: 'fill', value: { $param: 'var.label' } }, guide: 'wait-for-user' },
				{ id: 'c', do: { kind: 'none' }, guide: 'next' }
			])
		);
		expect(out.steps.map((s) => [s.do.kind, s.guide])).toEqual([
			['click', 'next'],
			['none', 'next'],
			['none', 'next']
		]);
	});

	it('leaves no registered guide step that typing ends or Enter cannot pass', () => {
		for (const j of publicJourneys.map(guided)) {
			for (const step of j.steps) {
				expect(step.guide, `${j.id}/${step.id}`).toBe('next');
				expect(step.do.kind, `${j.id}/${step.id}`).not.toBe('fill');
			}
		}
	});
});

describe('withFieldEnter', () => {
	const inner: Presenter = { show: vi.fn(), settle: vi.fn(), hide: vi.fn() };
	const step = {} as IRStep;

	function setup(human = true) {
		document.body.innerHTML = '<div id="t"><input id="in" /><textarea id="ta"></textarea></div><input id="out" />';
		const next = vi.fn();
		const p = withFieldEnter(inner);
		p.show(step, document.getElementById('t'), { human, next, action: { kind: 'none' } } as unknown as ShowCtx);
		const press = (id: string, init: KeyboardEventInit = {}) =>
			document.getElementById(id)!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, ...init }));
		return { p, next, press };
	}

	it('advances on Enter in the step\'s own field', () => {
		const { next, press } = setup();
		press('in');
		expect(next).toHaveBeenCalledOnce();
	});

	it('keeps Enter for newlines, chords and other fields', () => {
		const { next, press } = setup();
		press('ta');
		press('in', { shiftKey: true });
		press('in', { metaKey: true });
		press('out');
		expect(next).not.toHaveBeenCalled();
	});

	it('stops listening once the step is hidden, and never drives a scripted run', () => {
		const { p, next, press } = setup();
		p.hide();
		press('in');
		expect(next).not.toHaveBeenCalled();
		const scripted = setup(false);
		scripted.press('in');
		expect(scripted.next).not.toHaveBeenCalled();
	});
});
