import { describe, expect, it } from 'vitest';
import { compile, type Journey } from '@dorsk/journey';
import accountsPools from '../../../journeys/accounts-pools.journey';
import enrollMachine from '../../../journeys/enroll-machine.journey';
import searchSessions from '../../../journeys/search-sessions.journey';
import settingsTour from '../../../journeys/settings-tour.journey';
import spawnSession from '../../../journeys/spawn-session.journey';
import usageOverview from '../../../journeys/usage-overview.journey';

const SPECS: Journey[] = [
	accountsPools,
	enrollMachine,
	spawnSession,
	usageOverview,
	settingsTour,
	searchSessions
];
const FIXTURE_ONLY = /admin|acme-research|production|a0000000|Machines \d/;

describe('a public step is one a real user can finish', () => {
	it('navigates to its own route, so replaying from the guides page works', () => {
		for (const spec of SPECS) {
			for (const step of compile(spec, { public: true }).steps) {
				expect(step.route, `${spec.id}/${step.id}`).toBeTruthy();
			}
		}
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
