import type { SessionListItem } from '@bindings/SessionListItem';

/** Match only this dispatch on this machine, never a name/cwd/time heuristic.
 * Codex and OpenCode persist command_id as metadata.spawn_key when registering
 * their native session id. Claude Code already has a pre-minted id. */
export function findSpawnedSession<T extends Pick<SessionListItem, 'id' | 'machine_id' | 'metadata'>>(
	sessions: T[],
	machineId: string,
	commandId: string,
	sessionId: string | null
): T | null {
	return sessions.find((session) => {
		if (session.machine_id !== machineId) return false;
		if (sessionId) return session.id === sessionId;
		const meta = session.metadata;
		return meta !== null && typeof meta === 'object' && !Array.isArray(meta)
			&& meta.spawn_key === commandId;
	}) ?? null;
}
