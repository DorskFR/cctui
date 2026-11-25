// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import { setCapabilitiesForTest } from '#lib/features.svelte.ts';
import { PaneModel, type PaneInputs } from './paneModel.svelte';
import pane from '../ConversationPane.svelte?raw';

const { idle } = vi.hoisted(() => ({ idle: () => ({ data: undefined, isLoading: false }) }));

vi.mock('@tanstack/svelte-query', () => ({
	useQueryClient: () => ({ invalidateQueries: vi.fn(), setQueryData: vi.fn() })
}));

vi.mock('#lib/queries/index.ts', () => {
	const pending = () => new Promise(() => {});
	return {
		endpoints: new Proxy({}, { get: () => pending }),
		qk: new Proxy({}, { get: () => (...args: unknown[]) => ['k', ...args] }),
		useConversation: idle,
		useHarnesses: idle,
		useHarnessModels: idle,
		useLabels: idle,
		useMachines: idle,
		useMe: idle,
		usePlugins: idle,
		useVoiceConfig: idle,
		useBookmarks: idle,
		useBookmarkActions: () => ({ create: vi.fn(), update: vi.fn(), remove: vi.fn() }),
		useMessagePins: idle,
		useMessagePinActions: () => ({ pin: vi.fn(), unpin: vi.fn() }),
		useSessionActions: () => ({})
	};
});

vi.mock('#lib/queries/scheduled.ts', () => ({
	useScheduledMessages: idle,
	scheduledTurns: () => new Map()
}));

const session = (over: Partial<SessionListItem> = {}) =>
	({ id: 's1', status: 'active', adapter_id: 'claude-code', machine_id: 'm1', labels: [], ...over }) as unknown as SessionListItem;

let stop: (() => void) | undefined;
afterEach(() => {
	stop?.();
	stop = undefined;
	setCapabilitiesForTest(null);
});

function build(initial: { highlight?: string[]; focusSeq?: number | null } = {}) {
	const props = $state({
		opened: session(),
		highlight: initial.highlight ?? [],
		focusSeq: initial.focusSeq ?? null
	});
	const inputs: PaneInputs = {
		get opened() {
			return props.opened;
		},
		get highlight() {
			return props.highlight;
		},
		get focusSeq() {
			return props.focusSeq;
		},
		chrome: 'drawer'
	};
	const spies = { reset: vi.fn(), unstick: vi.fn(), openBar: vi.fn() };
	stop = $effect.root(() => {
		const p = new PaneModel(inputs);
		p.scroll.resetForSession = spies.reset;
		p.scroll.unstick = spies.unstick;
		p.search.openBar = spies.openBar;
	});
	flushSync();
	return { props, spies };
}

describe('pane effects track only the props they read', () => {
	it('does not reset the scroll when the list hands over a fresh copy of the same session', () => {
		const { props, spies } = build();
		expect(spies.reset).toHaveBeenCalledTimes(1);
		props.opened = session({ attention: 'needs_input' } as Partial<SessionListItem>);
		flushSync();
		expect(spies.reset).toHaveBeenCalledTimes(1);
		expect(spies.unstick).not.toHaveBeenCalled();
	});

	it('still follows a change of the search hit', () => {
		const { props, spies } = build();
		props.focusSeq = 7;
		flushSync();
		expect(spies.unstick).toHaveBeenCalledTimes(1);
	});

	it('does not re-seed the find bar on a fresh copy of the same session', () => {
		setCapabilitiesForTest({ features: ['transcript_search'], switchable: ['transcript_search'] });
		const { props, spies } = build({ highlight: ['needle'] });
		expect(spies.openBar).toHaveBeenCalledTimes(1);
		expect(spies.openBar).toHaveBeenCalledWith('needle');
		props.opened = session({ status: 'idle' } as Partial<SessionListItem>);
		flushSync();
		expect(spies.openBar).toHaveBeenCalledTimes(1);
	});

	it('is handed one getter per prop by the pane', () => {
		expect(pane).toContain('const p = new PaneModel({\n\t\tget opened() {');
		expect(pane).toContain('get focusSeq() {');
		expect(pane).toContain('get highlight() {');
	});
});
