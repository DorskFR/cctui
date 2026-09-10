import type { Journey, Text } from '@dorsk/journey';
import { publicJourneys, type StartOutcome, startGuide } from './journey';
import type { OnboardingSettings } from './settings.svelte';
import { settings } from './settings.svelte';

export type GuideStatus = 'done' | 'in-progress' | 'not-started';

export interface GuideEntry {
	id: string;
	version: number;
	title: string;
	description: string;
}

/** A message ref or per-locale map needs the mounted runtime to resolve it. */
export function guideText(text: Text | undefined): string {
	if (text === undefined) return '';
	if (typeof text === 'string') return text;
	return globalThis.window?.__journey?.translate(text) ?? '';
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

export function guideStatus(entry: GuideEntry, onboarding: OnboardingSettings): GuideStatus {
	if (progressJourneyId(onboarding.progress) === entry.id) return 'in-progress';
	return onboarding.seenVersion[entry.id] === entry.version ? 'done' : 'not-started';
}

export function clearGuide(id: string) {
	const current = settings.onboarding;
	const { [id]: _dropped, ...seenVersion } = current.seenVersion;
	settings.setOnboarding({
		seenVersion,
		progress: progressJourneyId(current.progress) === id ? null : current.progress
	});
}

export function resetGuides() {
	settings.setOnboarding({ seenVersion: {}, progress: null });
}

export async function replayGuide(id: string): Promise<StartOutcome> {
	clearGuide(id);
	return startGuide(id);
}
