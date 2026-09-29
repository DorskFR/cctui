import type { AgentEvent } from '@bindings/AgentEvent';
import { MSG_CATEGORIES } from './filters';
import { toLine, type LineBuildCtx } from './lines';
import type { MsgCategory } from './types';

export interface HeadHidden {
	/** Renderable rows before the first visible one, dropped by the filter. */
	count: number;
	/** Categories to switch on to reveal them. */
	categories: MsgCategory[];
}

export const NO_HEAD_HIDDEN: HeadHidden = { count: 0, categories: [] };

/** The render window reaches the oldest stored row: no page and no chunk left. */
export function atHeadOfTranscript(hiddenOlder: number, canFetchOlder: boolean): boolean {
	return hiddenOlder === 0 && !canFetchOlder;
}

// Markdown is not rendered while probing: the head of a take-over transcript can
// be hundreds of kilobytes and this runs on every filter change.
function probeCtx(ctx: LineBuildCtx, visible: (c: MsgCategory) => boolean): LineBuildCtx {
	const probe: LineBuildCtx = Object.create(ctx, {
		visible: { value: visible },
		renderMarkdown: { value: (s: string) => s },
		renderCode: { value: (text: string) => text }
	});
	return probe;
}

/** Count the renderable rows the filter hides ahead of the first visible one,
 *  so a transcript whose head is filtered away is not mistaken for a truncated
 *  page. Stops at the first visible row, so the cost is the hidden run. */
export function headHiddenByFilter(events: AgentEvent[], ctx: LineBuildCtx): HeadHidden {
	const current = probeCtx(ctx, (c) => ctx.visible(c));
	const unfiltered = probeCtx(ctx, () => true);
	let count = 0;
	const categories = new Set<MsgCategory>();
	for (const e of events) {
		if (toLine(e, current)) break;
		if (!toLine(e, unfiltered)) continue;
		count += 1;
		for (const c of MSG_CATEGORIES) {
			if (ctx.visible(c)) continue;
			if (toLine(e, probeCtx(ctx, (x) => x === c || ctx.visible(x)))) {
				categories.add(c);
				break;
			}
		}
	}
	return count === 0 ? NO_HEAD_HIDDEN : { count, categories: [...categories] };
}
