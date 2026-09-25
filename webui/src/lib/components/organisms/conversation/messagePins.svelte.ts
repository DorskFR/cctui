import type { AgentEvent } from '@bindings/AgentEvent';
import { useMessagePinActions, useMessagePins } from '$lib/queries';
import { createSeqJumper, type RenderWindow } from './jump';
import type { ScrollController } from './scroll.svelte';
import type { Line } from './types';

export interface MessagePinsOpts {
	id: () => string;
	events: () => AgentEvent[];
	canFetchOlder: () => boolean;
	fetchOlder: () => Promise<void>;
	scroll: ScrollController;
}

export class MessagePins {
	#o: MessagePinsOpts;
	#query: ReturnType<typeof useMessagePins>;
	actions = useMessagePinActions();
	renderWindow = $state<RenderWindow | undefined>(undefined);
	pins = $derived(this.#query.data ?? []);
	pinnedSeqs = $derived(new Set(this.pins.map((p) => p.seq)));
	ensureSeqVisible: (seq: number) => Promise<void>;

	constructor(o: MessagePinsOpts) {
		this.#o = o;
		this.#query = useMessagePins(() => o.id());
		this.ensureSeqVisible = createSeqJumper({
			hasSeq: (seq) => o.events().some((e) => e.seq === seq),
			isRendered: (seq) => this.renderWindow?.isRendered(seq) ?? false,
			growRender: () => this.renderWindow?.grow(),
			canFetchOlder: o.canFetchOlder,
			fetchOlder: o.fetchOlder,
			centerOnSeq: o.scroll.centerOnSeq,
			unstick: o.scroll.unstick
		}).ensureSeqVisible;
	}

	unpinSeq = (seq: number): void => {
		void this.actions.unpin(this.#o.id(), seq);
	};

	toggleLine = (ln: Line): void => {
		if (ln.seq === undefined || ln.pending || ln.failed) return;
		void (this.pinnedSeqs.has(ln.seq)
			? this.actions.unpin(this.#o.id(), ln.seq)
			: this.actions.pin(this.#o.id(), ln.seq, ln.messageId ?? null));
	};
}
