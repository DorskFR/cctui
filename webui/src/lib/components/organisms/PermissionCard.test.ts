// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import PermissionCard from './PermissionCard.svelte';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const base = {
	type: 'permission_request',
	session_id: 's1',
	request_id: 'call-1',
	tool_name: 'read',
	description: 'read',
	input_preview: '{"path":"."}'
};

async function render(req: object) {
	const onrespond = vi.fn();
	comp = mount(PermissionCard, { target: document.body, props: { req, onrespond } as never });
	await flush();
	return onrespond;
}

const buttons = () => [...document.querySelectorAll<HTMLButtonElement>('button')];

describe('PermissionCard', () => {
	it('keeps the two buttons for a native harness', async () => {
		const onrespond = await render(base);
		expect(buttons().map((b) => b.textContent?.trim())).toEqual(['Deny', 'Allow']);
		buttons()[1].click();
		expect(onrespond).toHaveBeenCalledWith('call-1', true);
	});

	it("renders the agent's own options in order", async () => {
		await render({
			...base,
			options: [
				{ option_id: 'a-always', name: 'Always allow', kind: 'allow_always' },
				{ option_id: 'a-once', name: 'Allow once', kind: 'allow_once' },
				{ option_id: 'r-once', name: 'Reject', kind: 'reject_once' },
				{ option_id: 'r-always', name: '', kind: 'reject_always' }
			]
		});
		expect(buttons().map((b) => b.textContent?.trim())).toEqual([
			'Always allow',
			'Allow once',
			'Reject',
			'reject_always'
		]);
	});

	it('answers with the picked option id and its polarity', async () => {
		const onrespond = await render({
			...base,
			options: [
				{ option_id: 'a-always', name: 'Always allow', kind: 'allow_always' },
				{ option_id: 'r-once', name: 'Reject', kind: 'reject_once' }
			]
		});
		buttons()[0].click();
		expect(onrespond).toHaveBeenLastCalledWith('call-1', true, 'a-always');
		buttons()[1].click();
		expect(onrespond).toHaveBeenLastCalledWith('call-1', false, 'r-once');
	});
});
