// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import MessageBubble, { roleColor } from './MessageBubble.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function render(props: Record<string, unknown>) {
	comp = mount(MessageBubble, { target: document.body, props: { role: 'assistant', ...props } });
	return document.querySelector('.bubble') as HTMLElement | null;
}

describe('MessageBubble', () => {
	it('renders markdown html in a prose bubble carrying its role', () => {
		const el = render({ html: '<p>hi</p>' });
		expect(el?.tagName).toBe('DIV');
		expect(el?.classList.contains('assistant')).toBe(true);
		expect(el?.querySelector('p')?.textContent).toBe('hi');
	});

	it('renders highlighted code in a mono pre for a tool call', () => {
		const el = render({ role: 'tool', mcp: true, htmlCode: '<span>ls</span>' });
		expect(el?.tagName).toBe('PRE');
		expect(el?.classList.contains('code')).toBe(true);
		expect(el?.classList.contains('mcp')).toBe(true);
	});

	it('falls back to plain text, and to nothing without a body', () => {
		expect(render({ role: 'result', text: '<b>raw</b>' })?.textContent).toBe('<b>raw</b>');
		unmount(comp!);
		comp = null;
		expect(render({})).toBeNull();
	});

	it('takes the role-tint and delivery-state classes', () => {
		const el = render({ role: 'user', html: '<p>x</p>', tinted: true, pending: true });
		expect(el?.classList.contains('tinted')).toBe(true);
		expect(el?.classList.contains('pending')).toBe(true);
	});

	it('gives every role its badge colour', () => {
		expect(roleColor('user')).toBe('var(--role-user)');
		expect(roleColor('result')).toBe('var(--role-tool)');
		expect(roleColor('tool', true)).toBe('var(--role-mcp)');
		expect(roleColor('unknown')).toBe('var(--text-muted)');
	});
});
