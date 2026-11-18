import type { RedteamCell } from '@bindings/RedteamCell';
import type { RedteamEscape } from '@bindings/RedteamEscape';
import type { RedteamOutcome } from '@bindings/RedteamOutcome';
import type { RedteamRun } from '@bindings/RedteamRun';
import type { RedteamVersion } from '@bindings/RedteamVersion';
import { m } from '#lib/paraglide/messages.js';

export type OutcomeTone = 'ok' | 'warn' | 'danger' | 'info' | 'neutral';

export function outcomeTone(o: RedteamOutcome): OutcomeTone {
	switch (o) {
		case 'contained':
		case 'blocked':
			return 'ok';
		case 'escaped':
			return 'danger';
		case 'inconclusive':
			return 'warn';
		case 'refused':
			return 'info';
		default:
			return 'neutral';
	}
}

const LABEL: Record<RedteamOutcome, () => string> = {
	contained: () => m.redteam_outcome_contained(),
	escaped: () => m.redteam_outcome_escaped(),
	refused: () => m.redteam_outcome_refused(),
	inconclusive: () => m.redteam_outcome_inconclusive(),
	blocked: () => m.redteam_outcome_blocked(),
	skipped: () => m.redteam_outcome_skipped()
};

export const outcomeLabel = (o: RedteamOutcome): string => LABEL[o]();

export function escapeLabel(e: RedteamEscape): string {
	if (!e.cell) return m.redteam_escape_run();
	return e.probe ? `${e.probe} · ${e.cell}` : e.cell;
}

/** Escaped cells first, then the rest in run order. */
export function cellsEscapedFirst(cells: readonly RedteamCell[]): RedteamCell[] {
	const escaped = cells.filter((c) => c.verdict === 'escaped');
	return [...escaped, ...cells.filter((c) => c.verdict !== 'escaped')];
}

export function probeSummary(cell: RedteamCell): string {
	return cell.probes.map((p) => `${p.id} ${outcomeLabel(p.verdict)}`).join(' · ');
}

export type LabelledRun = { key: string; label: string; run: RedteamRun };

export function versionRuns(v: RedteamVersion): LabelledRun[] {
	const runs: LabelledRun[] = [];
	if (v.t2) runs.push({ key: 't2', label: m.redteam_tier_t2(), run: v.t2 });
	for (const run of v.t3) {
		const lane = run.report.lane || '—';
		runs.push({ key: `t3:${lane}`, label: m.redteam_tier_t3({ lane }), run });
	}
	return runs;
}
