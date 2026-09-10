import type { MessagePin } from '@bindings/MessagePin';
import type { Line } from './types';
import { m } from '$lib/paraglide/messages';

const EXCERPT_MAX = 90;

// A pin can outlive the window the drawer has fetched, so the panel must render
// a row for a seq whose line is not loaded.
export function pinExcerpt(line: Line | undefined): string {
	const raw = (line?.text ?? '').replace(/\s+/g, ' ').trim();
	if (!raw) return line ? (line.tool ?? '') || m.conversation_pins_no_excerpt() : m.conversation_pins_no_excerpt();
	return raw.length > EXCERPT_MAX ? `${raw.slice(0, EXCERPT_MAX - 1)}…` : raw;
}

export function pinRole(line: Line | undefined): string {
	switch (line?.role) {
		case 'user':
			return 'user';
		case 'assistant':
			return 'assistant';
		case 'thinking':
			return 'thinking';
		case 'system':
			return 'system';
		case 'tool':
		case 'result':
			return 'tool';
		default:
			return 'boundary';
	}
}

export function isPinned(pins: MessagePin[] | undefined, seq: number | undefined): boolean {
	return seq !== undefined && !!pins?.some((p) => p.seq === seq);
}
