import { describe, expect, it } from 'vitest';
import { inboxBatchMarkdown, parseInboxBatch } from './inboxBatch';

const BATCH =
	'[cctui inbox: 2 events]\n' +
	'1. (n8n, 2026-11-16T09:02Z) ticket moved\n' +
	'2. (approval, 2026-11-16T09:07Z) approved: ship it\nsee 1. (fake, x) inside';

describe('parseInboxBatch', () => {
	it('reads the server marker and each numbered event', () => {
		expect(parseInboxBatch(BATCH)).toEqual({
			count: 2,
			events: [
				{ source: 'n8n', at: '2026-11-16T09:02Z', body: 'ticket moved' },
				{
					source: 'approval',
					at: '2026-11-16T09:07Z',
					body: 'approved: ship it\nsee 1. (fake, x) inside'
				}
			]
		});
	});

	it('takes the singular header', () => {
		expect(parseInboxBatch('[cctui inbox: 1 event]\n1. (timer, t) wake')?.events).toHaveLength(1);
	});

	it('refuses prose that only looks like the marker', () => {
		expect(parseInboxBatch('[cctui inbox: what is this')).toBeNull();
		expect(parseInboxBatch('about [cctui inbox: 2 events]')).toBeNull();
		expect(parseInboxBatch('[cctui keep-alive x]')).toBeNull();
	});
});

describe('inboxBatchMarkdown', () => {
	it('heads each event with its number, source and time', () => {
		const md = inboxBatchMarkdown(parseInboxBatch(BATCH)!);
		expect(md).toContain('**1. n8n** · 2026-11-16T09:02Z\n\nticket moved');
		expect(md).toContain('**2. approval** · 2026-11-16T09:07Z');
		expect(md).not.toContain('[cctui inbox');
	});
});
