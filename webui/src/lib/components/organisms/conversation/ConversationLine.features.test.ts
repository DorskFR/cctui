// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import type { Feature } from '@bindings/Feature';
import { setCapabilitiesForTest } from '$lib/features.svelte';
import ConversationLine from './ConversationLine.svelte';
import type { Line } from './types';

const attachments = vi.hoisted(() => vi.fn());

vi.mock('$lib/queries', async (orig) => ({
	...(await orig<typeof import('$lib/queries')>()),
	useSessionAttachments: (id: () => string) => {
		attachments(id());
		return { data: [], isLoading: false };
	}
}));

const SWITCHABLE: Feature[] = ['voice', 'uploads', 'plugins'];
const off = (...names: Feature[]) =>
	setCapabilitiesForTest({ features: SWITCHABLE.filter((f) => !names.includes(f)), switchable: SWITCHABLE });

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	setCapabilitiesForTest(null);
	attachments.mockClear();
});

async function render(ln: Line, extra: Record<string, unknown> = {}) {
	comp = mount(ConversationLine, {
		target: document.body,
		props: {
			ln,
			archived: false,
			sessionId: 's1',
			onretry: vi.fn(),
			onedit: vi.fn(),
			onsaveimage: vi.fn(),
			oncopymarkdown: vi.fn(),
			...extra
		}
	});
	await new Promise((r) => setTimeout(r, 0));
	return document.querySelector('.line') as HTMLElement;
}

const base = (over: Partial<Line> = {}): Line => ({
	role: 'assistant',
	ts: 1_700_000_000_000,
	html: '<p>spoken text</p>',
	text: 'spoken text',
	...over
});

describe('voice note on a line', () => {
	it('plays the note with voice on', async () => {
		Object.defineProperty(HTMLMediaElement.prototype, 'play', {
			value: vi.fn(async () => {}),
			configurable: true
		});
		off();
		const el = await render(base({ voiceNoteId: 'n1' }));
		expect(el.querySelector('[data-testid="voice-note"]')).not.toBeNull();
	});

	it('shows the text alone, with no player, without voice', async () => {
		off('voice');
		const el = await render(base({ voiceNoteId: 'n1' }));
		expect(el.querySelector('[data-testid="voice-note"]')).toBeNull();
		expect(el.querySelector('audio')).toBeNull();
		expect(el.textContent).toContain('spoken text');
	});
});

describe('uploads on a user line', () => {
	const uploaded = () =>
		base({ role: 'user', uploads: { sessionId: 's1', names: ['notes.txt'] } });

	it('looks the files up with uploads on', async () => {
		off();
		await render(uploaded());
		expect(attachments).toHaveBeenCalledWith('s1');
	});

	it('neither renders nor asks for them without uploads', async () => {
		off('uploads');
		const el = await render(uploaded());
		expect(attachments).not.toHaveBeenCalled();
		expect(el.querySelector('.attachments')).toBeNull();
	});
});

describe('plugin actions on a line', () => {
	const actions = [{ pluginId: 'p1', label: 'Open in board', icon: 'grid', params: {}, autoOpen: false }];

	it('renders the action with plugins on', async () => {
		off();
		const el = await render(base(), { pluginActions: actions, onpluginaction: vi.fn() });
		expect(el.textContent).toContain('Open in board');
	});

	it('renders nothing for plugins without plugins', async () => {
		off('plugins');
		const el = await render(base(), { pluginActions: actions, onpluginaction: vi.fn() });
		expect(el.textContent).not.toContain('Open in board');
	});
});
