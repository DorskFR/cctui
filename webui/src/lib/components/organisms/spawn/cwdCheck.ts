import type { MachineLiveness } from '@bindings/MachineLiveness';
import { ApiError, FeatureOffError } from '#lib/api.ts';
import { GIT_INFO_DEBOUNCE_MS } from './cwdGitInfo';

/** What the machine's daemon says about the spawn dialog's working directory. */
export type CwdCheck = 'unknown' | 'exists' | 'missing' | 'offline';

// The dirs route answers 404 for a path the daemon reports absent and 503 when
// no daemon is reachable; anything else says nothing about existence.
export function cwdCheckOf(err: unknown): CwdCheck {
	if (!(err instanceof ApiError) || err instanceof FeatureOffError) return 'unknown';
	if (err.status === 404) return 'missing';
	if (err.status === 503) return 'offline';
	return 'unknown';
}

// Debounced (machine, path) existence check; only the latest request may
// deliver. A machine known offline resolves to 'offline' without asking.
export function makeCwdCheckWatcher(
	lookup: (machineId: string, path: string) => Promise<unknown>,
	onResult: (check: CwdCheck) => void,
	delayMs = GIT_INFO_DEBOUNCE_MS
) {
	let timer: ReturnType<typeof setTimeout> | undefined;
	let seq = 0;
	function cancel() {
		seq++;
		if (timer !== undefined) clearTimeout(timer);
		timer = undefined;
	}
	function update(machineId: string, path: string, liveness: MachineLiveness | undefined) {
		cancel();
		const dir = path.trim();
		if (!machineId || !dir || !liveness) {
			onResult('unknown');
			return;
		}
		if (liveness === 'offline') {
			onResult('offline');
			return;
		}
		onResult('unknown');
		const mine = seq;
		timer = setTimeout(() => {
			timer = undefined;
			Promise.resolve()
				.then(() => lookup(machineId, dir))
				.then(
					() => {
						if (mine === seq) onResult('exists');
					},
					(err: unknown) => {
						if (mine === seq) onResult(cwdCheckOf(err));
					}
				);
		}, delayMs);
	}
	return { update, cancel };
}
