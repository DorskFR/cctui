import type { Line } from './types';

// Only assistant lines carry the `messageId` shared with the on-disk transcript.
export function forkableIds(lines: Line[]): string[] {
	return lines
		.filter((l) => l.role === 'assistant' && l.messageId)
		.map((l) => l.messageId as string);
}

export function forkRange(ids: string[], selected: Iterable<string>): string[] {
	const idxs = [...selected]
		.map((mid) => ids.indexOf(mid))
		.filter((i) => i >= 0)
		.sort((a, b) => a - b);
	if (idxs.length === 0) return [];
	return ids.slice(idxs[0], idxs[idxs.length - 1] + 1);
}

export class ForkSelection {
	active = $state(false);
	selected = $state<Set<string>>(new Set());

	toggleMode = (): void => {
		if (this.active) this.exit();
		else this.active = true;
	};

	toggle = (messageId: string): void => {
		const next = new Set(this.selected);
		if (next.has(messageId)) next.delete(messageId);
		else next.add(messageId);
		this.selected = next;
	};

	exit = (): void => {
		this.active = false;
		this.selected = new Set();
	};

	range(lines: Line[]): string[] {
		return forkRange(forkableIds(lines), this.selected);
	}
}
