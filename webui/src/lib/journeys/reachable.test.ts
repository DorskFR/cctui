import { describe, expect, it } from 'vitest';
import { compile, type Journey } from '@dorsk/journey';
import accountsPools from '../../../journeys/accounts-pools.journey';
import followSession from '../../../journeys/follow-session.journey';
import sessionsList from '../../../journeys/sessions-list.journey';
import welcome from '../../../journeys/welcome.journey';
import enrollMachine from '../../../journeys/enroll-machine.journey';
import searchSessions from '../../../journeys/search-sessions.journey';
import settingsTour from '../../../journeys/settings-tour.journey';
import spawnSession from '../../../journeys/spawn-session.journey';
import usageOverview from '../../../journeys/usage-overview.journey';

const SPECS: Journey[] = [
	welcome,
	sessionsList,
	accountsPools,
	enrollMachine,
	spawnSession,
	followSession,
	usageOverview,
	settingsTour,
	searchSessions
];
const FIXTURE_ONLY = /admin|acme-research|production|a0000000|Machines \d/;

describe('a public step is one a real user can finish', () => {
	it('opens on a route, so replaying from the guides page lands on the right page', () => {
		for (const spec of SPECS) {
			expect(compile(spec, { public: true }).steps[0]?.route, spec.id).toBeTruthy();
		}
	});

	it('declares a route only where it means to interrupt', () => {
		// In guide mode a route change is not silent: the runtime takes the user
		// away with a "Go to another page" card carrying no spotlight. Crossing by
		// letting them click the real control costs nothing, so a later step that
		// declares a route is a deliberate exception, not the default.
		const interrupts = SPECS.flatMap((spec) =>
			compile(spec, { public: true })
				.steps.slice(1)
				.filter((s) => s.route !== undefined)
				.map((s) => `${spec.id}/${s.id}`)
		);
		expect(interrupts).toEqual([
			'welcome/guides',
			'settings-tour/sessions',
			'settings-tour/harness',
			'settings-tour/patterns',
			'settings-tour/guides'
		]);
	});

	it('never waits on a name only the seed fixture has', () => {
		for (const spec of SPECS) {
			for (const step of compile(spec, { public: true }).steps) {
				const where = `${spec.id}/${step.id}`;
				expect(JSON.stringify(step.target ?? ''), where).not.toMatch(FIXTURE_ONLY);
				expect(JSON.stringify(step.expect ?? []), where).not.toMatch(FIXTURE_ONLY);
			}
		}
	});

	it('never waits on an English accessible name, which hangs under fr', () => {
		for (const spec of SPECS) {
			for (const step of compile(spec, { public: true }).steps) {
				for (const e of step.expect ?? []) {
					for (const v of Object.values(e)) {
						expect(
							typeof v === 'object' && v !== null && 'name' in v,
							`${spec.id}/${step.id}`
						).toBe(false);
					}
				}
			}
		}
	});
});
