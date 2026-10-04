// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { submitChord } from './platform';

const key = (init: KeyboardEventInit) =>
	new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init });

describe('submitChord', () => {
	it('submits on a chord bubbling from a descendant and consumes it only when acted on', () => {
		const node = document.createElement('div');
		const field = node.appendChild(document.createElement('textarea'));
		let ready = false;
		const submit = vi.fn(() => ready);
		const action = submitChord(node, submit);

		const refused = key({ key: 'Enter', ctrlKey: true });
		field.dispatchEvent(refused);
		expect(submit).toHaveBeenCalledTimes(1);
		expect(refused.defaultPrevented).toBe(false);

		ready = true;
		const taken = key({ key: 'Enter', metaKey: true });
		field.dispatchEvent(taken);
		expect(taken.defaultPrevented).toBe(true);

		field.dispatchEvent(key({ key: 'Enter' }));
		expect(submit).toHaveBeenCalledTimes(2);

		action.destroy();
		field.dispatchEvent(key({ key: 'Enter', ctrlKey: true }));
		expect(submit).toHaveBeenCalledTimes(2);
	});
});
