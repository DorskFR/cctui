// Report journeys whose screens a change set can affect but did not refresh.
// Advisory only: capture runs on a developer's machine against a seeded stack,
// so CI cannot regenerate anything — it can only notice that nobody did.
//
//   node scripts/journey-staleness.mjs --base origin/main
//
// Prints a markdown report, or nothing when the record is up to date.
import { execFileSync } from 'node:child_process';
import { journeysForChanges } from './journeys-for-changes.mjs';

const argv = process.argv.slice(2);
const filesFlag = argv.indexOf('--files');
const base = argv[argv.indexOf('--base') + 1] ?? 'origin/main';
const changed =
	filesFlag >= 0
		? argv.slice(filesFlag + 1).filter((a) => !a.startsWith('--'))
		: execFileSync('git', ['diff', '--name-only', `${base}...HEAD`], { encoding: 'utf8' })
				.split('\n')
				.filter(Boolean);

const expected = journeysForChanges(changed);
const refreshed = new Set(
	changed
		.map((f) => f.match(/^docs\/journeys\/([^/]+)\//)?.[1])
		.filter(Boolean)
);
const stale = expected.filter((id) => !refreshed.has(id));

if (!stale.length) process.exit(0);

console.log(
	[
		'#### Journey screens may be stale',
		'',
		`This branch touches the web UI, but ${stale.length === 1 ? 'this journey has' : 'these journeys have'} no refreshed screens:`,
		'',
		...stale.map((id) => `- \`${id}\``),
		'',
		'Regenerate with a seeded local stack, then commit what changed:',
		'',
		'```sh',
		'make local/demo',
		`cd webui && npm run journey:shoot -- ${stale.join(' ')}`,
		'```',
		'',
		'Nothing is blocked by this — the record is refreshed by hand, on purpose.'
	].join('\n')
);
