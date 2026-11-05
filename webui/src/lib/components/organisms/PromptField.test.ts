// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createRawSnippet, flushSync, mount, unmount } from 'svelte';

vi.mock('$lib/queries', () => ({ useSessions: () => ({ data: { sessions: [] } }) }));

import PromptField from './PromptField.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const tools = createRawSnippet(() => ({
	render: () => '<button type="button" class="probe">Schedule</button>'
}));

describe('PromptField tools slot', () => {
	it('renders no toolbar without tools', () => {
		comp = mount(PromptField, { target: document.body, props: { value: '' } });
		flushSync();
		expect(document.querySelector('textarea')).not.toBeNull();
		expect(document.querySelector('[role="toolbar"]')).toBeNull();
	});

	it('renders the tools in a toolbar under the field', () => {
		comp = mount(PromptField, { target: document.body, props: { value: '', tools } });
		flushSync();
		const bar = document.querySelector('[role="toolbar"]');
		expect(bar?.querySelector('.probe')).not.toBeNull();
		const textarea = document.querySelector('textarea');
		expect(textarea).not.toBeNull();
		expect(
			textarea && bar ? textarea.compareDocumentPosition(bar) & Node.DOCUMENT_POSITION_FOLLOWING : 0
		).toBeTruthy();
	});
});
