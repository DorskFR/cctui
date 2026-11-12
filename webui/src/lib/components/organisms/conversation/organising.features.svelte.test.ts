// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { setCapabilitiesForTest } from '$lib/features.svelte';
import { MessagePins } from './messagePins.svelte';
import { BookmarkSaver } from './bookmarkSave.svelte';
import type { ScrollController } from './scroll.svelte';
import type { Line } from './types';
import pane from '../ConversationPane.svelte?raw';
import header from './DrawerHeader.svelte?raw';
import notice from './AutoArchiveNotice.svelte?raw';

const calls = vi.hoisted(() => ({
	pinsEnabled: [] as boolean[],
	pin: vi.fn(async () => {}),
	unpin: vi.fn(async () => {})
}));

vi.mock('$lib/queries', () => ({
	useMessagePins: (_id: () => string, enabled: () => boolean = () => true) => {
		calls.pinsEnabled.push(enabled());
		return { data: undefined };
	},
	useMessagePinActions: () => ({ pin: calls.pin, unpin: calls.unpin }),
	useBookmarks: () => ({ data: [] }),
	useBookmarkActions: () => ({ create: vi.fn(), update: vi.fn(), remove: vi.fn() })
}));

const organising = (on: boolean) =>
	setCapabilitiesForTest({ features: on ? ['organising'] : [], switchable: ['organising'] });

const line = { seq: 4, role: 'assistant', text: 'hello' } as unknown as Line;
let stop: (() => void) | undefined;

function build() {
	let pins!: MessagePins;
	let saver!: BookmarkSaver;
	stop = $effect.root(() => {
		pins = new MessagePins({
			id: () => 's1',
			events: () => [],
			canFetchOlder: () => false,
			fetchOlder: async () => {},
			scroll: { centerOnSeq: vi.fn(), unstick: vi.fn() } as unknown as ScrollController
		});
		saver = new BookmarkSaver({ id: () => 's1', sessionName: () => null });
	});
	flushSync();
	return { pins, saver };
}

beforeEach(() => {
	calls.pinsEnabled = [];
	calls.pin.mockClear();
	calls.unpin.mockClear();
});
afterEach(() => {
	stop?.();
	stop = undefined;
	setCapabilitiesForTest(null);
});

describe('message pins and bookmarks without organising', () => {
	it('fetch no pins', () => {
		organising(false);
		const { pins } = build();
		expect(calls.pinsEnabled.every((e) => e === false)).toBe(true);
		expect(pins.pins).toEqual([]);
	});

	it('pin, unpin and open a bookmark draft for nothing', () => {
		organising(false);
		const { pins, saver } = build();
		pins.toggleLine(line);
		pins.unpinSeq(4);
		saver.open(line);
		expect(calls.pin).not.toHaveBeenCalled();
		expect(calls.unpin).not.toHaveBeenCalled();
		expect(saver.draft).toBeNull();
	});

	it('fetch and pin with organising on', () => {
		organising(true);
		const { pins } = build();
		expect(calls.pinsEnabled).toContain(true);
		pins.toggleLine(line);
		expect(calls.pin).toHaveBeenCalledWith('s1', 4, null);
	});
});

const prop = (src: string, tag: string, name: string) => {
	const markup = src.slice(src.indexOf('</script>'));
	const start = markup.indexOf(`<${tag}`);
	const body = markup.slice(start, markup.indexOf('/>', start));
	const at = body.indexOf(`${name}=`);
	return at < 0 ? '' : body.slice(at, body.indexOf('\n', at));
};

describe('conversation pane organising gates', () => {
	it('wires the star, label picker, pins and bookmarks only with organising', () => {
		expect(prop(pane, 'DrawerHeader', 'onTogglePin')).toContain('organising ?');
		expect(prop(pane, 'DrawerHeader', 'onAttachLabel')).toContain('organising ?');
		expect(prop(pane, 'DrawerToolbar', 'onjumpseq')).toContain('organising ?');
		expect(prop(pane, 'DrawerToolbar', 'onunpin')).toContain('organising ?');
		expect(prop(pane, 'Conversation\n', 'onpin')).toContain('organising ?');
		expect(prop(pane, 'Conversation\n', 'onbookmark')).toContain('organising ?');
		expect(prop(pane, 'AutoArchiveNotice', 'onpin')).toContain('organising ?');
	});

	it('lists an admin machines only with machine spawn', () => {
		const at = pane.indexOf('const myMachines = useMachines(');
		expect(pane.slice(at, pane.indexOf(');', at))).toContain("hasFeature('machine_spawn')");
	});

	it('shows header label chips only with organising', () => {
		expect(header).toContain("const showLabels = $derived(hasFeature('organising'));");
		expect(header).toContain('{#if showLabels && session.labels.length === 0 && labelEditable}');
		expect(header).toContain('{#if showLabels && session.labels.length > 0 && !compact}');
	});

	it('drops the keep action when no pin handler is given', () => {
		expect(notice).toMatch(/\{#if onpin\}\s*<Button[^>]*onclick=\{onpin\}/);
	});
});
