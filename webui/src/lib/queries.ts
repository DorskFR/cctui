import { createQuery, useQueryClient } from '@tanstack/svelte-query';
import { toStore } from 'svelte/store';
import { api } from './api';
import type { SessionListResponse } from '@bindings/SessionListResponse';
import type { SessionStats } from '@bindings/SessionStats';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { AgentEvent } from '@bindings/AgentEvent';
import type { SpawnRequest } from '@bindings/SpawnRequest';
import type { SpawnResponse } from '@bindings/SpawnResponse';
import type { DispatchRequest } from '@bindings/DispatchRequest';
import type { DispatchResponse } from '@bindings/DispatchResponse';
import type { UserRow } from '@bindings/UserRow';
import type { MachineRow } from '@bindings/MachineRow';
import type { UserTokenRow } from '@bindings/UserTokenRow';
import type { CreateUserResponse } from '@bindings/CreateUserResponse';
import type { RotateResponse } from '@bindings/RotateResponse';
import type { MintTokenResponse } from '@bindings/MintTokenResponse';
import type { VersionInfo } from '@bindings/VersionInfo';

/** Machine kinds the server manages itself — the per-user `dispatch` machine
 * and one-shot `ephemeral` worker pods. They are never spawn targets and are
 * hidden from the "new machines" list in the UI (CCT-183 / CCT-185). */
export const SYSTEM_MACHINE_KINDS = new Set(['dispatch', 'ephemeral']);

/** Centralised query keys so invalidation stays consistent. */
export const qk = {
	version: ['version'] as const,
	sessions: (archived: boolean) => ['sessions', { archived }] as const,
	sessionStats: ['session-stats'] as const,
	// NOT under ['sessions'] on purpose: list invalidations (`['sessions']`,
	// bumped ~every 2s while streaming) must NOT refetch the conversation —
	// a refetched history that overlaps the live ws events produced duplicate
	// messages. Live updates come through the ws listener, not refetch.
	conversation: (id: string) => ['conversation', id] as const,
	users: ['users'] as const,
	machines: (userId: string) => ['users', userId, 'machines'] as const,
	tokens: (userId: string) => ['users', userId, 'tokens'] as const
};

/** Raw typed fetchers — also usable outside of components. */
export const endpoints = {
	version: () => api.get<VersionInfo>('/version'),
	sessions: (archived: boolean) =>
		api.get<SessionListResponse>('/sessions', { include_archived: archived || undefined }),
	/** Aggregate session counts for the Overview — correct past the list's
	 * 25-row display cap (the list-derived counts are not). */
	sessionStats: () => api.get<SessionStats>('/sessions/stats'),
	// Full-transcript substring search (CCT-184). `includeArchived` sets scope
	// (live-only vs all); an empty `q` with `includeArchived` browses the
	// archive. Offset-paginated.
	searchSessions: (q: string, includeArchived: boolean, limit: number, offset: number) =>
		api.get<SessionListResponse>('/sessions/search', {
			q: q || undefined,
			include_archived: includeArchived || undefined,
			limit,
			offset
		}),
	session: (id: string) => api.get<SessionListItem>(`/sessions/${id}`),
	conversation: (id: string) => api.get<AgentEvent[]>(`/sessions/${id}/conversation`),
	recentDirs: (machineId: string) =>
		api.get<string[]>('/sessions/recent-dirs', { machine_id: machineId || undefined }),
	users: () => api.get<UserRow[]>('/admin/users'),
	machines: (userId: string) => api.get<MachineRow[]>(`/admin/users/${userId}/machines`),
	tokens: (userId: string) => api.get<UserTokenRow[]>(`/admin/users/${userId}/tokens`),
	/** Spawn on a machine. Always `multipart/form-data` (CCT-203): the JSON
	 *  `SpawnRequest` rides in the `request` part and any attached files ride as
	 *  file parts the daemon stages under /tmp/cctui-uploads. */
	spawn: (body: SpawnRequest, files: File[] = []) => {
		const form = new FormData();
		form.append('request', JSON.stringify(body));
		for (const f of files) form.append('files', f, f.name);
		return api.postForm<SpawnResponse>('/sessions/spawn', form);
	},
	dispatch: (body: DispatchRequest) => api.post<DispatchResponse>('/sessions/dispatch', body),
	/** Configured dispatcher ids (e.g. `["claude-worker"]`); empty when none. */
	dispatchers: () => api.get<string[]>('/sessions/dispatchers'),
	/** Every spawnable machine across all active users — for the spawn picker.
	 * Excludes server-managed machines (`ephemeral` worker pods and the per-user
	 * `dispatch` machine): those aren't somewhere you'd start an interactive
	 * session, only real enrolled daemons are (CCT-183 / CCT-185). */
	allMachines: async (): Promise<MachineRow[]> => {
		const users = (await api.get<UserRow[]>('/admin/users')).filter((u) => !u.revoked_at);
		const lists = await Promise.all(
			users.map((u) => api.get<MachineRow[]>(`/admin/users/${u.id}/machines`))
		);
		return lists.flat().filter((m) => !m.revoked_at && !SYSTEM_MACHINE_KINDS.has(m.kind));
	}
};

/* ---------------- Queries ----------------
 * This svelte-query build types options as `T | Readable<T>` (not an accessor
 * function), so reactive params are bridged from runes via Svelte 5's
 * `toStore(getter)`; param-less queries pass a plain options object. */

export const useVersion = () =>
	createQuery({ queryKey: qk.version, queryFn: endpoints.version, staleTime: 60_000 });

export const useSessions = (archived: () => boolean) =>
	createQuery(
		toStore(() => ({
			queryKey: qk.sessions(archived()),
			queryFn: () => endpoints.sessions(archived()),
			refetchInterval: 15_000
		}))
	);

export const useSessionStats = () =>
	createQuery({
		queryKey: qk.sessionStats,
		queryFn: endpoints.sessionStats,
		refetchInterval: 15_000
	});

export const useConversation = (id: () => string, enabled: () => boolean = () => true) =>
	createQuery(
		toStore(() => ({
			queryKey: qk.conversation(id()),
			queryFn: () => endpoints.conversation(id()),
			enabled: enabled() && !!id()
		}))
	);

export const useRecentDirs = (machineId: () => string) =>
	createQuery(
		toStore(() => ({
			queryKey: ['recent-dirs', machineId()],
			queryFn: () => endpoints.recentDirs(machineId()),
			enabled: !!machineId(),
			staleTime: 30_000
		}))
	);

export const useUsers = () => createQuery({ queryKey: qk.users, queryFn: endpoints.users });

export const useDispatchers = (enabled: () => boolean) =>
	createQuery(
		toStore(() => ({
			queryKey: ['dispatchers'],
			queryFn: endpoints.dispatchers,
			enabled: enabled(),
			staleTime: 60_000
		}))
	);

export const useAllMachines = (enabled: () => boolean) =>
	createQuery(
		toStore(() => ({
			queryKey: ['machines', 'all'],
			queryFn: endpoints.allMachines,
			enabled: enabled()
		}))
	);

export const useMachines = (userId: () => string, enabled: () => boolean) =>
	createQuery(
		toStore(() => ({
			queryKey: qk.machines(userId()),
			queryFn: () => endpoints.machines(userId()),
			enabled: enabled()
		}))
	);

export const useTokens = (userId: () => string, enabled: () => boolean) =>
	createQuery(
		toStore(() => ({
			queryKey: qk.tokens(userId()),
			queryFn: () => endpoints.tokens(userId()),
			enabled: enabled()
		}))
	);

/* ---------------- Actions (plain async + invalidation) ----------------
 * These are intentionally NOT createMutation: they return promises so callers
 * can await + toast, and they invalidate the relevant queries directly. Must
 * be called during component init (they read the query-client context). */

/** Build a placeholder card for an in-flight dispatch (CCT-193). Mirrors the
 * fields the worker will report once its daemon registers, so the optimistic
 * card looks like the real one until the refetch reconciles it by id. */
function optimisticDispatchCard(id: string, body: DispatchRequest): SessionListItem {
	const p = (body.payload ?? {}) as Record<string, string>;
	return {
		id,
		parent_id: null,
		machine_id: 'dispatch',
		// Real cwd is unknown until the worker registers; show the target repo if
		// the payload carries one, else nothing (no `dispatch:<origin>` noise).
		working_dir: p.repo ?? '',
		status: 'new',
		liveness: 'stale',
		attention: null,
		bucket: 'working',
		uptime_secs: 0,
		token_usage: {
			tokens_in: 0,
			tokens_out: 0,
			cost_usd: 0,
			cache_read_tokens: 0,
			cache_creation_tokens: 0
		},
		metadata: null,
		adapter_id: 'claude-code',
		machine_name: 'dispatch',
		machine_hue: null,
		last_message_text: 'Dispatching…',
		last_message_at: null,
		name: p.name || p.prompt_file || (p.prompt ? p.prompt.slice(0, 40) : null) || id.slice(0, 6),
		model: p.model ?? null,
		effort: p.effort ?? null,
		auto_approve: false,
		match_snippet: null,
		last_activity_at: null,
		cache_cold: false,
		estimated_burst_tokens: null,
		hibernated: false
	};
}

export function useSessionActions() {
	const qc = useQueryClient();
	const inval = () => qc.invalidateQueries({ queryKey: ['sessions'] });
	return {
		rename: async (id: string, name: string) => {
			await api.patch<void>(`/sessions/${id}`, { name });
			inval();
		},
		archive: async (id: string) => {
			await api.post<void>(`/sessions/${id}/archive`);
			inval();
		},
		unarchive: async (id: string) => {
			await api.post<void>(`/sessions/${id}/unarchive`);
			inval();
		},
		// Batch archive/unarchive (CCT-172). One request, one invalidation.
		archiveMany: async (ids: string[]) => {
			if (ids.length === 0) return;
			await api.post<void>('/sessions/archive', { ids });
			inval();
		},
		unarchiveMany: async (ids: string[]) => {
			if (ids.length === 0) return;
			await api.post<void>('/sessions/unarchive', { ids });
			inval();
		},
		kill: async (id: string) => {
			await api.post<void>(`/sessions/${id}/kill`);
			inval();
		},
		interrupt: async (id: string) => {
			await api.post<void>(`/sessions/${id}/interrupt`);
		},
		setAutoApprove: async (id: string, enabled: boolean) => {
			await api.post<void>(`/sessions/${id}/auto-approve`, { enabled });
			inval();
		},
		spawn: (body: SpawnRequest, files: File[] = []) => endpoints.spawn(body, files),
		// Dispatch returns synchronously (no daemon ACK / command_id), so unlike
		// spawn there's nothing to await on the ws — the worker pod registers the
		// pre-minted session_id later. We optimistically insert a placeholder card
		// keyed by the client-minted session_id so the list updates IMMEDIATELY
		// (CCT-193); the eventual refetch reconciles it by id (the worker pod, or
		// the server's `failed` row on a backend error, both carry the same id).
		dispatch: async (body: DispatchRequest) => {
			const key = qk.sessions(false);
			const id = body.session_id ?? crypto.randomUUID();
			if (body.session_id == null) body = { ...body, session_id: id };
			const placeholder = optimisticDispatchCard(id, body);
			qc.setQueryData<SessionListResponse>(key, (prev) => ({
				sessions: [placeholder, ...(prev?.sessions ?? []).filter((s) => s.id !== id)]
			}));
			try {
				const res = await endpoints.dispatch(body);
				inval();
				return res;
			} catch (e) {
				// Reconcile to server truth (the row exists as `failed`); the card
				// stays visible so the user can see + retry the failed dispatch.
				inval();
				throw e;
			}
		}
	};
}

export function useUserActions() {
	const qc = useQueryClient();
	const invalUsers = () => qc.invalidateQueries({ queryKey: qk.users });
	const invalUser = (userId: string) => qc.invalidateQueries({ queryKey: ['users', userId] });
	return {
		create: async (name: string): Promise<CreateUserResponse> => {
			const r = await api.post<CreateUserResponse>('/admin/users', { name });
			invalUsers();
			return r;
		},
		rename: async (id: string, name: string) => {
			await api.patch<void>(`/admin/users/${id}`, { name });
			invalUsers();
		},
		setCanDispatch: async (id: string, canDispatch: boolean) => {
			await api.patch<void>(`/admin/users/${id}`, { can_dispatch: canDispatch });
			invalUsers();
		},
		rotate: async (id: string): Promise<RotateResponse> => {
			const r = await api.post<RotateResponse>(`/admin/users/${id}/rotate`);
			invalUsers();
			return r;
		},
		revoke: async (id: string) => {
			await api.del<void>(`/admin/users/${id}`);
			invalUsers();
		},
		purgeUser: async (id: string) => {
			await api.del<void>(`/admin/users/${id}/purge`);
			invalUsers();
		},
		mintToken: async (userId: string, label: string | null): Promise<MintTokenResponse> => {
			const r = await api.post<MintTokenResponse>(`/users/${userId}/tokens`, { label });
			invalUser(userId);
			return r;
		},
		relabelToken: async (userId: string, tokenId: string, label: string | null) => {
			await api.patch<void>(`/admin/users/${userId}/tokens/${tokenId}`, { label });
			invalUser(userId);
		},
		revokeToken: async (userId: string, tokenId: string) => {
			await api.del<void>(`/admin/users/${userId}/tokens/${tokenId}`);
			invalUser(userId);
		},
		purgeToken: async (userId: string, tokenId: string) => {
			await api.del<void>(`/admin/users/${userId}/tokens/${tokenId}/purge`);
			invalUser(userId);
		},
		rotateMachine: async (userId: string, id: string): Promise<RotateResponse> => {
			const r = await api.post<RotateResponse>(`/admin/machines/${id}/rotate`);
			invalUser(userId);
			return r;
		},
		// The PATCH replaces both fields (display_name + hue), so callers pass
		// the full pair — send the current value for the field they didn't touch.
		updateMachine: async (
			userId: string,
			id: string,
			displayName: string | null,
			hue: number | null
		) => {
			await api.patch<void>(`/admin/machines/${id}`, { display_name: displayName, hue });
			invalUser(userId);
		},
		revokeMachine: async (userId: string, id: string) => {
			await api.del<void>(`/admin/machines/${id}`);
			invalUser(userId);
		},
		purgeMachine: async (userId: string, id: string) => {
			await api.del<void>(`/admin/machines/${id}/purge`);
			invalUser(userId);
		}
	};
}
