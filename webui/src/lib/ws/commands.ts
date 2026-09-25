import { spawnOutcomeFromEnd, type CommandOutcome, type SpawnProbeHit } from './frames';

/** Daemon handshake budget (45 s) plus dispatch and relay slack. */
export const SPAWN_ACK_TIMEOUT_MS = 75_000;
/** How often `awaitSpawn` re-derives the outcome from the session list. */
export const SPAWN_PROBE_INTERVAL_MS = 5_000;

/** Outstanding command acks and spawns awaiting their outcome. */
export class CommandWaiters {
	private waiters = new Map<string, (r: CommandOutcome) => void>();
	/** Spawns waiting on their pre-minted session id to show up on this socket. */
	private spawnWaiters = new Map<string, (r: CommandOutcome) => void>();

	/** Settle the waiter for a `command_result` frame, if one is pending. */
	resolveCommand(commandId: string, r: CommandOutcome) {
		const w = this.waiters.get(commandId);
		if (w) {
			w(r);
			this.waiters.delete(commandId);
		}
	}

	/** Resolve when the server reports a result for `commandId` (spawn).
	 *
	 * The daemon fails a codex handshake at 45 s, so the wait outlasts that
	 * plus dispatch latency: a real failure must arrive here, not after the
	 * caller gave up. A timeout is still NOT a failure: a cold spawn
	 * (kickstarting the agent daemon, staging uploads) can outlive any
	 * client-side wait, and the session still lands. `timedOut` lets the caller
	 * phrase it as "unconfirmed, check the list" instead of an error inviting a
	 * retry — re-submitting dispatches a brand-new spawn and a duplicate agent.
	 */
	awaitCommand(commandId: string, timeoutMs = SPAWN_ACK_TIMEOUT_MS): Promise<CommandOutcome> {
		return new Promise((resolve) => {
			const timer = setTimeout(() => {
				if (this.waiters.delete(commandId)) {
					resolve({ ok: false, timedOut: true, error: 'no spawn confirmation from the daemon' });
				}
			}, timeoutMs);
			this.waiters.set(commandId, (r) => {
				clearTimeout(timer);
				resolve(r);
			});
		});
	}

	settleSpawn(sessionId: string | undefined, r: CommandOutcome) {
		if (!sessionId) return;
		const w = this.spawnWaiters.get(sessionId);
		if (!w) return;
		this.spawnWaiters.delete(sessionId);
		w(r);
	}

	/** Resolve a spawn on whichever lands first: the daemon's `command_result`
	 * for `commandId`, any event for the pre-minted `sessionId` on this
	 * socket, or `probe` finding the session's row. The ack is the only one of
	 * those the server never replays, so the other two cover a lost or late
	 * frame; the timeout is the last resort and still not a failure (see
	 * `awaitCommand`). */
	awaitSpawn(
		commandId: string,
		sessionId: string | null | undefined,
		opts: {
			probe?: () => Promise<SpawnProbeHit | null>;
			probeIntervalMs?: number;
			timeoutMs?: number;
		} = {}
	): Promise<CommandOutcome> {
		if (!sessionId) return this.awaitCommand(commandId, opts.timeoutMs);
		const interval = opts.probeIntervalMs ?? SPAWN_PROBE_INTERVAL_MS;
		return new Promise((resolve) => {
			let settled = false;
			let probeTimer: ReturnType<typeof setTimeout> | null = null;
			const finish = (r: CommandOutcome) => {
				if (settled) return;
				settled = true;
				this.waiters.delete(commandId);
				this.spawnWaiters.delete(sessionId);
				if (probeTimer) clearTimeout(probeTimer);
				resolve(r);
			};
			void this.awaitCommand(commandId, opts.timeoutMs).then(finish);
			this.spawnWaiters.set(sessionId, finish);
			const { probe } = opts;
			if (!probe) return;
			const tick = async () => {
				if (settled) return;
				let hit: SpawnProbeHit | null = null;
				try {
					hit = await probe();
				} catch {
					hit = null;
				}
				if (settled) return;
				if (hit) {
					finish(spawnOutcomeFromEnd(hit.end_reason ?? null, hit.end_detail ?? null));
					return;
				}
				probeTimer = setTimeout(() => void tick(), interval);
			};
			probeTimer = setTimeout(() => void tick(), interval);
		});
	}
}
