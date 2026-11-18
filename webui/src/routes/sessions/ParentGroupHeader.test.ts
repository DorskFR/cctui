// @vitest-environment happy-dom
import { flushSync, mount, unmount, type ComponentProps } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { SessionParentStub } from '@bindings/SessionParentStub';
import ParentGroupHeader from './ParentGroupHeader.svelte';

const parent = (over: Partial<SessionParentStub> = {}): SessionParentStub => ({
	id: 'orch-1',
	name: 'orch-x',
	working_dir: '/w/repo',
	status: 'active',
	...over
});

let app: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (app) unmount(app);
	app = null;
	document.body.innerHTML = '';
});

type Props = ComponentProps<typeof ParentGroupHeader>;

function render(over: Partial<Props> = {}) {
	const ontoggle = vi.fn();
	const onopenparent = vi.fn();
	const props: Props = { parent: parent(), count: 12, open: false, ontoggle, onopenparent, ...over };
	app = mount(ParentGroupHeader, { target: document.body, props });
	flushSync();
	return { ontoggle, onopenparent };
}

describe('ParentGroupHeader', () => {
	it('shows the parent, its live state and the collapsed child count', () => {
		const { ontoggle, onopenparent } = render();
		const text = document.body.textContent ?? '';
		expect(text).toContain('orch-x');
		expect(text).not.toContain('live');
		expect(document.querySelector('.slot [title]')?.getAttribute('title')).toBe('live');
		expect(text).toContain('12 subagents');
		const buttons = [...document.querySelectorAll('button')];
		const toggle = buttons.find((b) => b.hasAttribute('aria-expanded'));
		expect(toggle?.getAttribute('aria-expanded')).toBe('false');
		toggle?.click();
		expect(ontoggle).toHaveBeenCalledOnce();
		buttons.find((b) => b.textContent?.includes('orch-x'))?.click();
		expect(onopenparent).toHaveBeenCalledWith('orch-1');
	});

	it('marks an archived parent and a single child', () => {
		render({ parent: parent({ status: 'archived' }), count: 1, open: true });
		const text = document.body.textContent ?? '';
		expect(text).not.toContain('archived');
		expect(document.querySelector('.slot [title]')?.getAttribute('title')).toBe('archived');
		expect(text).toMatch(/\b1 subagent\b/);
		expect(document.querySelector('[aria-expanded]')?.getAttribute('aria-expanded')).toBe('true');
	});

	it('uses the live session dot when the parent row is loaded', () => {
		const session = {
			id: 'orch-1',
			status: 'active',
			liveness: 'active',
			bucket: 'working',
			working_dir: '/w/repo',
			machine_id: 'm1',
			token_usage: { tokens_in: 0, tokens_out: 0, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0 },
			metadata: {}
		} as unknown as SessionListItem;
		render({ session });
		expect(document.querySelector('.slot .dot.dot-active')).not.toBeNull();
	});
});
