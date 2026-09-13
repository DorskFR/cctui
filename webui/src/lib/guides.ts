import type { Journey, Text } from '@dorsk/journey';
import { guideDone, publicJourneys, type StartOutcome, startGuide } from './journey';
import type { OnboardingSettings } from './settings.svelte';
import { settings } from './settings.svelte';

export type GuideStatus = 'done' | 'in-progress' | 'not-started';

export type GuideSectionId = 'basics' | 'setup' | 'run' | 'master';

export interface GuideEntry {
	id: string;
	version: number;
	title: string;
	description: string;
}

export interface CurriculumEntry {
	/** journey id, or 'intro' for the carousel which is not an anchored journey */
	id: string;
	section: GuideSectionId;
	/** position within the section */
	order: number;
	/** guide ids that must be done before this one unlocks */
	requires: string[];
	xp: number;
}

export const GUIDE_SECTIONS: readonly GuideSectionId[] = ['basics', 'setup', 'run', 'master'];

/** Teaching order: read the app, configure it, run work, then go deeper. */
export const CURRICULUM: readonly CurriculumEntry[] = [
	{ id: 'welcome', section: 'basics', order: 1, requires: [], xp: 10 },
	{ id: 'sessions-list', section: 'basics', order: 2, requires: [], xp: 15 },
	{ id: 'accounts-pools', section: 'setup', order: 1, requires: [], xp: 20 },
	{ id: 'enroll-machine', section: 'setup', order: 2, requires: [], xp: 20 },
	{ id: 'spawn-session', section: 'run', order: 1, requires: ['enroll-machine'], xp: 25 },
	{ id: 'follow-session', section: 'run', order: 2, requires: ['spawn-session'], xp: 30 },
	{ id: 'search-sessions', section: 'master', order: 1, requires: [], xp: 20 },
	{ id: 'usage-overview', section: 'master', order: 2, requires: [], xp: 20 },
	{ id: 'settings-tour', section: 'master', order: 3, requires: [], xp: 20 }
];

export interface GuideView extends GuideEntry {
	section: GuideSectionId;
	xp: number;
	status: GuideStatus;
	locked: boolean;
	/** Titles of the guides that unlock this one, never raw ids. */
	lockedBy: string[];
	step: { index: number; total: number } | null;
}

export interface GuideSectionView {
	id: GuideSectionId;
	locked: boolean;
	lockedBy: string[];
	guides: GuideView[];
}

export interface CurriculumView {
	sections: GuideSectionView[];
	earnedXp: number;
	totalXp: number;
	doneCount: number;
	totalCount: number;
}

/** A message ref or per-locale map needs the mounted runtime to resolve it. */
export function guideText(text: Text | undefined): string {
	if (text === undefined) return '';
	if (typeof text === 'string') return text;
	const runtime = globalThis.window?.__journey;
	return typeof runtime?.translate === 'function' ? runtime.translate(text) : '';
}

/** The compiled book still carries journeys whose public step count is zero. */
export function guideEntries(source: Journey[] = publicJourneys as Journey[]): GuideEntry[] {
	return source.map((j) => ({
		id: j.id,
		version: j.version ?? 1,
		title: guideText(j.title) || j.id,
		description: guideText(j.description)
	}));
}

export function progressJourneyId(progress: string | null): string | null {
	if (!progress) return null;
	try {
		const parsed: unknown = JSON.parse(progress);
		const id = (parsed as { id?: unknown } | null)?.id;
		return typeof id === 'string' && id ? id : null;
	} catch {
		return null;
	}
}

/** `done` overrides the stored marker for the guides whose completion is read
 *  from live state rather than written on finish. */
export function guideStatus(
	entry: GuideEntry,
	onboarding: OnboardingSettings,
	done?: boolean
): GuideStatus {
	if (progressJourneyId(onboarding.progress) === entry.id) return 'in-progress';
	if (done === true || onboarding.seenVersion[entry.id] === entry.version) return 'done';
	const step = (onboarding.stepProgress ?? {})[entry.id];
	if (step && step.version === entry.version && step.index > 0) return 'in-progress';
	return 'not-started';
}

/** Live-state completion for every guide that writes no done marker. */
export async function guideDoneMap(ids: string[]): Promise<Record<string, boolean>> {
	const out: Record<string, boolean> = {};
	await Promise.all(
		ids.map(async (id) => {
			try {
				out[id] = await guideDone(id);
			} catch {
				out[id] = false;
			}
		})
	);
	return out;
}

function sectionRank(id: GuideSectionId): number {
	return GUIDE_SECTIONS.indexOf(id);
}

/** Group the available guides into the curriculum, resolving every lock into
 *  the titles that open it. A curriculum id this build ships no journey for is
 *  left out entirely, so it can never wall off the sections behind it. */
export function buildCurriculum(
	entries: GuideEntry[],
	onboarding: OnboardingSettings,
	done: Record<string, boolean> = {}
): CurriculumView {
	const byId = new Map(entries.map((e) => [e.id, e]));
	const rows = CURRICULUM.filter((c) => byId.has(c.id))
		.slice()
		.sort((a, b) => sectionRank(a.section) - sectionRank(b.section) || a.order - b.order);

	const status = new Map(rows.map((c) => [c.id, guideStatus(byId.get(c.id) as GuideEntry, onboarding, done[c.id])]));
	const isDone = (id: string) => status.get(id) === 'done';
	const titleOf = (id: string) => byId.get(id)?.title ?? id;

	const sections: GuideSectionView[] = [];
	let earnedXp = 0;
	let totalXp = 0;
	let doneCount = 0;

	for (const sectionId of GUIDE_SECTIONS) {
		const members = rows.filter((c) => c.section === sectionId);
		if (!members.length) continue;

		const blockers = rows
			.filter((c) => sectionRank(c.section) < sectionRank(sectionId) && !isDone(c.id))
			.map((c) => c.id);

		const guides = members.map((c) => {
			const entry = byId.get(c.id) as GuideEntry;
			const st = status.get(c.id) as GuideStatus;
			const lockedIds = [...blockers, ...c.requires.filter((r) => byId.has(r) && !isDone(r))];
			const stored = (onboarding.stepProgress ?? {})[c.id];
			totalXp += c.xp;
			if (st === 'done') {
				earnedXp += c.xp;
				doneCount += 1;
			}
			return {
				...entry,
				section: sectionId,
				xp: c.xp,
				status: st,
				locked: st !== 'done' && lockedIds.length > 0,
				lockedBy: [...new Set(lockedIds)].map(titleOf),
				step:
					st !== 'done' && stored && stored.version === entry.version && stored.index > 0
						? { index: stored.index, total: stored.total }
						: null
			} satisfies GuideView;
		});

		sections.push({
			id: sectionId,
			locked: blockers.length > 0,
			lockedBy: [...new Set(blockers)].map(titleOf),
			guides
		});
	}

	return { sections, earnedXp, totalXp, doneCount, totalCount: rows.length };
}

export function clearGuide(id: string) {
	const current = settings.onboarding;
	const { [id]: _dropped, ...seenVersion } = current.seenVersion;
	const { [id]: _step, ...stepProgress } = current.stepProgress ?? {};
	settings.setOnboarding({
		seenVersion,
		stepProgress,
		progress: progressJourneyId(current.progress) === id ? null : current.progress
	});
}

export function resetGuides() {
	settings.setOnboarding({ seenVersion: {}, progress: null, stepProgress: {} });
}

/** Replaying drops the done marker so the guide reads as unfinished while it
 *  runs, but a refused start must leave the marker where it was. */
export async function replayGuide(id: string): Promise<StartOutcome> {
	const previous = settings.onboarding.seenVersion[id];
	clearGuide(id);
	const outcome = await startGuide(id);
	if (!outcome.ok && previous !== undefined) {
		settings.setOnboarding({ seenVersion: { ...settings.onboarding.seenVersion, [id]: previous } });
	}
	return outcome;
}
