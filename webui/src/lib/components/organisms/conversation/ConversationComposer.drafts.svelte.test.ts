// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { SessionListItem } from '@bindings/SessionListItem';
import { composerKey, setDraftRemote } from '#lib/drafts.ts';
import type { ScrollController } from './scroll.svelte';
import ConversationComposer from './ConversationComposer.svelte';

vi.mock('#lib/queries/index.ts', async (importOriginal) => {
	const empty = () => ({ data: undefined });
	return {
		...(await importOriginal<typeof import('#lib/queries/index.ts')>()),
		useSessionAttachments: empty,
		useScheduledMessages: empty,
		useScheduledActions: () => ({}),
		useSessions: empty,
		useMe: empty,
		useVoiceConfig: empty
	};
});

const A = 'aaaaaaaa-0000-0000-0000-000000000001';
const B = 'bbbbbbbb-0000-0000-0000-000000000002';
const session = (id: string) => ({ id, adapter_id: 'claude-code', status: 'active' }) as SessionListItem;

const remote = { put: vi.fn(), del: vi.fn() };
let comp: ReturnType<typeof mount> | null = null;

beforeEach(() => {
	localStorage.clear();
	setDraftRemote(remote);
});
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	setDraftRemote(null);
	vi.clearAllMocks();
	document.body.innerHTML = '';
});

function render(id: string, carriesDraft?: (from: string, to: string) => boolean) {
	const props = $state({
		session: session(id),
		archived: false,
		lastAgentAt: null,
		supportsAttachments: false,
		scroll: { textarea: null } as unknown as ScrollController,
		onsend: vi.fn(),
		stageFiles: async () => ({ paths: [] }),
		onNewFromScript: vi.fn(),
		carriesDraft
	});
	comp = mount(ConversationComposer, { target: document.body, props });
	flushSync();
	const box = () => document.querySelector('textarea') as HTMLTextAreaElement;
	return {
		props,
		box,
		type(text: string) {
			box().value = text;
			box().dispatchEvent(new Event('input', { bubbles: true }));
			flushSync();
		}
	};
}

describe('ConversationComposer drafts across a session switch', () => {
	it('shows the other session its own draft and leaves both drafts intact', () => {
		localStorage.setItem(composerKey(B), 'B half-written');
		const c = render(A);
		c.type('A text');
		c.props.session = session(B);
		flushSync();
		expect(c.box().value).toBe('B half-written');
		expect(localStorage.getItem(composerKey(A))).toBe('A text');
		expect(localStorage.getItem(composerKey(B))).toBe('B half-written');
		expect(remote.put).not.toHaveBeenCalledWith(composerKey(B), 'A text');
	});

	it('never deletes the other session draft when leaving an empty box', () => {
		localStorage.setItem(composerKey(B), 'B half-written');
		const c = render(A);
		c.props.session = session(B);
		flushSync();
		expect(localStorage.getItem(composerKey(B))).toBe('B half-written');
		expect(remote.del).not.toHaveBeenCalledWith(composerKey(B));
	});

	it('moves the typed draft along a follow-up swap', () => {
		const c = render(A, () => true);
		c.type('an unsent draft');
		c.props.session = session(B);
		flushSync();
		expect(c.box().value).toBe('an unsent draft');
		expect(localStorage.getItem(composerKey(B))).toBe('an unsent draft');
		expect(localStorage.getItem(composerKey(A))).toBeNull();
	});

	it('a swap onto a session with its own draft keeps that draft', () => {
		localStorage.setItem(composerKey(B), 'the head draft');
		const c = render(A, () => true);
		c.type('older text');
		c.props.session = session(B);
		flushSync();
		expect(c.box().value).toBe('the head draft');
		expect(localStorage.getItem(composerKey(B))).toBe('the head draft');
	});
});
