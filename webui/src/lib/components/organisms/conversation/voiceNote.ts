export const VOICE_TOOL = /(^|__)CctuiSpeak$/;

const FRESH_MS = 60_000;
const started = new Set<string>();

export function voiceNoteUrl(sessionId: string, noteId: string): string {
  return `/api/v1/sessions/${encodeURIComponent(sessionId)}/voice-notes/${encodeURIComponent(noteId)}`;
}

/** A note announces itself once, and only when it arrived live rather than on reload. */
export function claimAnnounce(noteId: string, ts: number, now = Date.now()): boolean {
  if (started.has(noteId) || now - ts > FRESH_MS) return false;
  started.add(noteId);
  return true;
}

export function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const s = Math.floor(seconds);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
