import type { Line } from './types';

const norm = (s: string) => s.replace(/\s+/g, ' ').trim();

/** Whether the live transcript is already in the conversation as the latest
 *  spoken user turn, so the voice bar need not repeat it. */
export function voiceTranscriptLanded(lines: readonly Line[], transcript: string): boolean {
	const heard = norm(transcript);
	if (!heard) return false;
	for (let i = lines.length - 1; i >= 0; i--) {
		const ln = lines[i];
		if (ln.role !== 'user') continue;
		return !!ln.voice && norm(ln.text ?? '') === heard;
	}
	return false;
}
