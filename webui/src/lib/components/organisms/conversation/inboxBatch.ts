/** Must match the server's `inbox::deliver::MARKER`. */
export const INBOX_MARKER = '[cctui inbox:';

export interface InboxBatchEvent {
	source: string;
	at: string;
	body: string;
}

export interface InboxBatch {
	count: number;
	events: InboxBatchEvent[];
}

const HEAD_RE = /^\[cctui inbox: (\d+) events?\]$/;
const ITEM_RE = /^(\d+)\. \(([^,()]+), ([^()]+)\) ?/;

export function parseInboxBatch(text: string): InboxBatch | null {
	const trimmed = text.trimStart();
	if (!trimmed.startsWith(INBOX_MARKER)) return null;
	const nl = trimmed.indexOf('\n');
	const head = HEAD_RE.exec((nl === -1 ? trimmed : trimmed.slice(0, nl)).trim());
	if (!head) return null;
	const events: InboxBatchEvent[] = [];
	for (const line of nl === -1 ? [] : trimmed.slice(nl + 1).split('\n')) {
		const item = ITEM_RE.exec(line);
		if (item && Number(item[1]) === events.length + 1) {
			events.push({ source: item[2], at: item[3], body: line.slice(item[0].length) });
		} else if (events.length) {
			events[events.length - 1].body += `\n${line}`;
		}
	}
	return { count: Number(head[1]), events };
}

export function inboxBatchMarkdown(batch: InboxBatch): string {
	return batch.events
		.map((e, i) => `**${i + 1}. ${e.source}** · ${e.at}\n\n${e.body.trim()}`)
		.join('\n\n');
}
