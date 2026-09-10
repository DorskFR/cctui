import { defineJourney } from '@dorsk/journey';

/**
 * WHAT'S-NEW CONTRACT: shipping a notable feature means bumping `version` here
 * and rewriting the closing card. The done marker is keyed `id@version`, so a
 * bump re-shows the deck exactly once to everyone who saw the previous one;
 * leaving the version alone means nobody ever sees the new copy.
 *
 * No step names a target, which is what routes the whole spec to the carousel
 * presenter in `src/lib/welcomeDeck.svelte.ts` rather than the anchored overlay.
 */
export default defineJourney({
	id: 'welcome',
	version: 1,
	title: 'Welcome to cctui',
	description: 'What cctui is and what each screen is for, in eight cards.',
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
				body: 'One control room for every Claude Code session you run. Start work, watch it progress, answer it when it asks, and see what it cost — from any browser, on any machine you enrolled.'
			}
		},
		{
			id: 'sessions',
			guide: 'next',
			say: {
				title: 'The session list',
				body: 'Every session, grouped by what it needs from you. The ones waiting on an answer float to the top; sort, filter and search narrow the rest.'
			}
		},
		{
			id: 'machines',
			guide: 'next',
			say: {
				title: 'Machines and the fleet',
				body: 'Sessions run on machines you enrol with a single command. The fleet view shows which are online and what each is carrying, so you can place work where there is room.'
			}
		},
		{
			id: 'accounts',
			guide: 'next',
			say: {
				title: 'Accounts and pools',
				body: 'Claude accounts live in pools. A session draws from a pool rather than a fixed account, so a rate-limited one steps aside instead of blocking the queue.'
			}
		},
		{
			id: 'spawn',
			guide: 'next',
			say: {
				title: 'Spawning work',
				body: 'Spawn opens a session from a prompt: pick the machine, the repository and the pool, attach files if it needs them, and it starts in the background.'
			}
		},
		{
			id: 'follow',
			guide: 'next',
			say: {
				title: 'Following a session',
				body: 'Open a session to read the conversation as it streams, reply when it asks a question, and interrupt or archive it when it is done.'
			}
		},
		{
			id: 'usage',
			guide: 'next',
			say: {
				title: 'Usage and cost',
				body: 'Tokens by hour, day, week and month, split by model, so an expensive habit shows up well before the bill does.'
			}
		},
		{
			id: 'guides',
			guide: 'next',
			say: {
				title: 'That is the tour',
				body: 'Each screen has a short guide that walks you through it in place. They are listed under Settings > Guides whenever you want one.'
			}
		}
	]
});
