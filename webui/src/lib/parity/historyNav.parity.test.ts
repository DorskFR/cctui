// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { HistoryNav } from '$lib/historyNav';
import { parityFixture } from './fixtures';

type Step =
	| { op: 'caret'; at: number }
	| { op: 'reset' }
	| { op: 'resetAll' }
	| { op: 'recall'; pick: string; value: string }
	| { op: 'expectBrowsing'; browsing: boolean }
	| { op: 'key'; key: string; handled: boolean; value: string };

type Case = { name: string; list: string[]; initial: string; steps: Step[] };

const fx = parityFixture<Case[]>('historyNav');

describe('historyNav parity fixtures', () => {
	for (const c of fx) {
		it(c.name, () => {
			let value = c.initial;
			let caret = value.length;
			const el = {
				get selectionStart() {
					return caret;
				},
				get selectionEnd() {
					return caret;
				}
			} as HTMLTextAreaElement;
			const nav = new HistoryNav({
				list: () => c.list,
				value: () => value,
				setValue: (v) => {
					value = v;
					caret = v.length;
				},
				el: () => el
			});
			for (const step of c.steps) {
				switch (step.op) {
					case 'caret':
						caret = step.at;
						break;
					case 'reset':
						nav.reset();
						break;
					case 'resetAll':
						nav.resetAll();
						break;
					case 'recall':
						nav.recall(step.pick);
						expect(value).toBe(step.value);
						break;
					case 'expectBrowsing':
						expect(nav.browsing).toBe(step.browsing);
						break;
					case 'key': {
						const handled = nav.handleKey({
							key: step.key,
							preventDefault: () => {}
						} as unknown as KeyboardEvent);
						expect(handled).toBe(step.handled);
						expect(value).toBe(step.value);
						break;
					}
				}
			}
		});
	}
});
