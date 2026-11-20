import type { AgentEvent } from '@bindings/AgentEvent';
import { highlightBlock, renderMarkdown } from '#lib/markdown.ts';
import { highlightTerms } from '#lib/search.ts';
import { headHiddenByFilter } from './headHidden';
import { createLineBuilder, type DeliveryState, type LineBuildCtx } from './lines';
import { threadLines, type ThreadSegment } from './threadView';
import type { Line, MsgCategory, ViewOpts } from './types';

export const isMachineUuid = (v: string) =>
	/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(v);

export interface LineRendererOpts {
	id: () => string;
	machineId: () => string;
	view: () => ViewOpts;
	highlight: () => string[];
	events: () => AgentEvent[];
	scheduledTurns: () => LineBuildCtx['scheduledTurns'];
	pending: () => DeliveryState['pending'];
	failed: () => DeliveryState['failed'];
	retrying: () => DeliveryState['retrying'];
	askPreamble: () => string | null | undefined;
	planPreamble: () => string | null | undefined;
	/** Earlier generations of a follow-up line, rendered above `events`. */
	prior?: () => readonly ThreadSegment[];
}

export class LineRenderer {
	#o: LineRendererOpts;
	#build = createLineBuilder();
	#segmentBuilds = new Map<string, ReturnType<typeof createLineBuilder>>();
	#ctx: LineBuildCtx;

	constructor(o: LineRendererOpts) {
		this.#o = o;
		// Getters, not snapshots: the toggles are read at build time so `lines`
		// re-runs when they flip.
		this.#ctx = {
			visible: (c: MsgCategory) => o.view().msgFilter[c],
			renderMarkdown: this.#markdown,
			renderCode: (text, lang) => this.hl(highlightBlock(text, lang)),
			get prettyJson() {
				return o.view().prettyJson;
			},
			get prettyDiff() {
				return o.view().prettyDiff;
			},
			get scheduledTurns() {
				return o.scheduledTurns();
			},
			get renderKey() {
				return `${o.view().prettyTables}|${o.id()}|${o.machineId()}`;
			}
		};
	}

	// Search terms to highlight inline, set when opened from a search.
	hl = (html: string): string => {
		const terms = this.#o.highlight();
		return terms.length ? highlightTerms(html, terms) : html;
	};

	// Local file paths link to the machine read-file route only when
	// `machine_id` is the machine UUID (daemon sessions); legacy
	// hostname-valued rows keep paths as text.
	#markdown = (s: string): string => {
		const machineId = this.#o.machineId();
		return this.hl(
			renderMarkdown(s, {
				tables: this.#o.view().prettyTables,
				sessionId: this.#o.id(),
				machineId: isMachineUuid(machineId) ? machineId : undefined
			})
		);
	};

	/** One memoized builder per session, so a live head event never re-renders
	 *  the earlier generations. */
	segmentLines = (seg: Pick<ThreadSegment, 'sessionId' | 'events'>): Line[] => {
		let build = this.#segmentBuilds.get(seg.sessionId);
		if (!build) {
			build = createLineBuilder();
			this.#segmentBuilds.set(seg.sessionId, build);
		}
		return build(seg.events, this.#ctx);
	};

	priorLines: Line[] = $derived.by(() => {
		const segments = this.#o.prior?.() ?? [];
		const keep = new Set(segments.map((s) => s.sessionId));
		for (const id of this.#segmentBuilds.keys()) if (!keep.has(id)) this.#segmentBuilds.delete(id);
		return threadLines(segments, this.segmentLines);
	});

	lines: Line[] = $derived.by(() =>
		this.#build(this.#o.events(), this.#ctx, {
			pending: this.#o.pending(),
			failed: this.#o.failed(),
			retrying: this.#o.retrying()
		})
	);
	// What the filter drops ahead of the first rendered row, so the head of the
	// transcript is never silently empty.
	headHidden = $derived.by(() => headHiddenByFilter(this.#o.events(), this.#ctx));
	askPreambleHtml = $derived.by(() => {
		const p = this.#o.askPreamble();
		return p ? this.hl(renderMarkdown(p)) : null;
	});
	planPreambleHtml = $derived.by(() => {
		const p = this.#o.planPreamble();
		return p ? this.hl(renderMarkdown(p)) : null;
	});
}
