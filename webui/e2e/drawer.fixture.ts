import type { Page } from '@playwright/test';
import { session, stubApi, transcript } from './tiles.fixture';

export const SESSIONS = Array.from({ length: 30 }, (_, i) => session(i));

/** The AskUserQuestion shape that blanked the drawer: two single-select
 *  questions, every option described, no previews. */
export const ASK = {
	questions: [
		{
			header: 'Restore',
			question: 'Restore the previous layout?',
			multiSelect: false,
			options: [
				{ label: 'Restore', description: 'Bring the previous layout back' },
				{ label: 'Keep', description: 'Keep the current layout' }
			]
		},
		{
			header: 'Fix',
			question: 'How should the overflow be fixed?',
			multiSelect: false,
			options: [
				{ label: 'Clip', description: 'Clip the panel to the viewport' },
				{ label: 'Scroll', description: 'Scroll only the transcript' },
				{ label: 'Both', description: 'Clip the panel and scroll the transcript' }
			]
		}
	]
};

/** Stubs the API for a drawer opened on a long transcript, optionally ending
 *  on a pending AskUserQuestion form. */
export function stubDrawer(page: Page, opts: { ask?: boolean } = {}) {
	return stubApi(page, SESSIONS, {
		conversation: (id) => {
			const events = transcript(id) as { ts: number; seq: number }[];
			if (!opts.ask) return events;
			const last = events[events.length - 1];
			return [
				...events,
				{ type: 'tool_call', tool: 'AskUserQuestion', input: ASK, ts: last.ts + 1000, seq: last.seq + 1 }
			];
		}
	});
}

/** How far each element can scroll vertically; the drawer panel must report 0. */
export const verticalOverflow = (page: Page, selector: string) =>
	page.evaluate(
		(sel) => [...document.querySelectorAll(sel)].map((el) => el.scrollHeight - el.clientHeight),
		selector
	);
