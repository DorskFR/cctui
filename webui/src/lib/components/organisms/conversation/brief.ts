import type { Line } from './types';

// The brief is the first thing the human typed. Adapter preambles, harness
// reminders and boundary markers all arrive as `system`/`marker`/`reset` lines
// and must not be mistaken for it.
export function firstUserLine(lines: Line[]): Line | undefined {
	return lines.find((l) => l.role === 'user' && !!(l.text ?? '').trim());
}

const SUMMARY_MAX = 140;

export function briefSummary(line: Line | undefined): string {
	const raw = (line?.text ?? '').replace(/\s+/g, ' ').trim();
	if (!raw) return '';
	return raw.length > SUMMARY_MAX ? `${raw.slice(0, SUMMARY_MAX - 1)}…` : raw;
}
