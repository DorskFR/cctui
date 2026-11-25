import type { EventRecord } from '@bindings/EventRecord';

export type FeedItem =
	| { type: 'row'; event: EventRecord }
	| { type: 'fold'; key: string; events: EventRecord[]; machineId: string | null };

function detail(ev: EventRecord, key: string): unknown {
	const d = ev.detail;
	return d !== null && typeof d === 'object' && !Array.isArray(d) ? (d as Record<string, unknown>)[key] : undefined;
}

export function isChildCreation(ev: EventRecord): boolean {
	return ev.kind === 'session.created' && detail(ev, 'origin') === 'child';
}

function parentOf(ev: EventRecord): string | null {
	const p = detail(ev, 'parent_session_id');
	return typeof p === 'string' ? p : null;
}

/** Adjacent child-session creations under the same parent collapse into one item. */
export function foldChildCreations(rows: EventRecord[], min = 2): FeedItem[] {
	const out: FeedItem[] = [];
	let run: EventRecord[] = [];
	const flush = () => {
		if (run.length >= min) {
			const machine = run[0].machine_id;
			out.push({
				type: 'fold',
				key: `fold-${run[0].id}`,
				events: run,
				machineId: run.every((e) => e.machine_id === machine) ? machine : null
			});
		} else {
			for (const event of run) out.push({ type: 'row', event });
		}
		run = [];
	};
	for (const ev of rows) {
		if (isChildCreation(ev) && (run.length === 0 || parentOf(run[0]) === parentOf(ev))) {
			run.push(ev);
			continue;
		}
		flush();
		if (isChildCreation(ev)) run.push(ev);
		else out.push({ type: 'row', event: ev });
	}
	flush();
	return out;
}
