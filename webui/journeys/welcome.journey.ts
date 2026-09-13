import { defineJourney } from '@dorsk/journey';

/**
 * WHAT'S-NEW CONTRACT: shipping a notable feature means bumping `version` here
 * and rewriting the closing card. The done marker is keyed `id@version`, so a
 * bump re-shows the deck exactly once to everyone who saw the previous one;
 * leaving the version alone means nobody ever sees the new copy.
 *
 * No step names a target, which is what routes the whole spec to the carousel
 * presenter in `src/lib/welcomeDeck.svelte.ts` rather than the anchored overlay.
 * It is also what lets this deck run on an install with no account, no machine
 * and no session: nothing here resolves against live DOM.
 */
export default defineJourney({
	id: 'welcome',
	version: 2,
	title: 'What cctui is',
	description: 'What cctui is and what every screen is for, as a carousel.',
	route: '/',
	autostart: { route: '/', once: true },
	level: 'smoke',
	variants: { viewport: ['desktop', 'mobile'], theme: ['dark'] },
	steps: [
		{
			id: 'what',
			guide: 'next',
			say: {
				title: 'Welcome to cctui',
				body: 'One control room for every Claude Code session you run. Start work, watch it progress, answer it when it asks, and see what it cost — from any browser, without opening a terminal on the machine doing the work.'
			}
		},
		{
			id: 'shape',
			guide: 'next',
			say: {
				title: 'How the pieces fit',
				body: 'Machines you enrol supply the compute. Provider accounts supply the quota. A session borrows one of each and runs a prompt to completion. Everything else in the app exists to set those up, watch them, or pay for them.'
			}
		},
		{
			id: 'overview',
			guide: 'next',
			say: {
				title: 'Overview — is the fleet healthy right now?',
				body: 'The landing page answers one question before you touch anything: what is live, what is stuck waiting on a human, which machines are up, and how the last thirty days have trended. Start here when you do not yet know where to look.'
			}
		},
		{
			id: 'sessions',
			guide: 'next',
			say: {
				title: 'Sessions — where the work actually happens',
				body: 'Every run you have started, grouped by what it needs from you. Sessions blocked on an answer surface first, because they are the only ones that stop making progress without you. Search, filters and display options narrow the rest once the list outgrows a screen.'
			}
		},
		{
			id: 'spawn',
			guide: 'next',
			say: {
				title: 'Starting a session',
				body: 'Spawn turns a prompt into a running agent: choose the machine, the repository and the account pool, attach anything it needs to read, and it works in the background whether or not you keep the tab open.'
			}
		},
		{
			id: 'follow',
			guide: 'next',
			say: {
				title: 'Following one in flight',
				body: 'Open a session to read its conversation as it streams. Reply when it asks, filter down to just the tool calls to audit what it touched, and interrupt or archive it when it is done or going the wrong way.'
			}
		},
		{
			id: 'accounts',
			guide: 'next',
			say: {
				title: 'Accounts — where the quota comes from',
				body: 'Provider accounts are grouped into pools, and a session draws from a pool rather than a fixed account, so one hitting a rate limit steps aside instead of stalling the queue. This page is also where GitHub connectors and dispatchers are wired up.'
			}
		},
		{
			id: 'access',
			guide: 'next',
			say: {
				title: 'Access — who and what may run work',
				body: 'Machines join the fleet from here with a single enrolment command, and users get tokens, scopes and the right to dispatch. If a session cannot find anywhere to run, or a colleague cannot sign in, this is the page to open.'
			}
		},
		{
			id: 'bookmarks',
			guide: 'next',
			say: {
				title: 'Bookmarks — the bits worth keeping',
				body: 'Any message from any conversation can be saved here with a note, then searched and copied out as markdown later. It is where the answer you will want again next month goes, instead of back into a session you will archive.'
			}
		},
		{
			id: 'review',
			guide: 'next',
			say: {
				title: 'Review — agents on your pull requests',
				body: 'When the review backend is deployed and a GitHub connector is set up, cctui reads diffs inline, collects draft comments and lets an agent reason about a specific line before you post anything.'
			}
		},
		{
			id: 'usage',
			guide: 'next',
			say: {
				title: 'Usage — what the fleet costs',
				body: 'Tokens by hour, day, week and month, split by model and by account, with pool gauges showing how close each is to its ceiling. An expensive habit shows up here well before it shows up on a bill.'
			}
		},
		{
			id: 'settings',
			guide: 'next',
			say: {
				title: 'Settings — tuning the defaults',
				body: 'Appearance and theme, how sessions behave and what they are permitted to do, privacy and retention, notifications, monitoring, security and instance-wide options. Change it once here rather than per session.'
			}
		},
		{
			id: 'guides',
			guide: 'next',
			say: {
				title: 'Now walk it in place',
				body: 'That is every screen. The rest of the guides run inside the app itself, pointing at the real controls in order — connect an account, enrol a machine, start a session, follow it. They are listed under Settings > Guides whenever you want one.'
			}
		}
	]
});
