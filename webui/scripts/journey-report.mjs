// The pull-request note about the visual record: which screens this branch
// moved, and which ones it arguably should have moved but did not.
//
//   node scripts/journey-report.mjs --base origin/main --sha <head> --repo o/r
//   node scripts/journey-report.mjs --files a.svelte docs/journeys/x/y/01.png
//
// Advisory only: capture needs a seeded stack, so CI can never regenerate
// anything — it can only report. Prints markdown, or nothing when there is
// neither a changed screen nor a stale one.
import { execFileSync } from 'node:child_process';
import { journeysForChanges } from './journeys-for-changes.mjs';

const argv = process.argv.slice(2);
const flag = (name) => {
	const i = argv.indexOf(`--${name}`);
	return i >= 0 ? argv[i + 1] : undefined;
};
const filesFlag = argv.indexOf('--files');
const base = flag('base') ?? 'origin/main';
const sha = flag('sha');
const repo = flag('repo');
// Enough to show what moved without burying the thread; a shared-token change
// legitimately moves all 74.
const GALLERY_MAX = 12;

const changed =
	filesFlag >= 0
		? argv.slice(filesFlag + 1).filter((a) => !a.startsWith('--'))
		: execFileSync('git', ['diff', '--name-only', `${base}...HEAD`], { encoding: 'utf8' })
				.split('\n')
				.filter(Boolean);

const screens = changed.filter((f) => /^docs\/journeys\/.+\.png$/.test(f));
const refreshed = new Set(screens.map((f) => f.split('/')[2]));
const stale = journeysForChanges(changed).filter((id) => !refreshed.has(id));

const out = [];

if (screens.length && sha && repo) {
	const shown = screens.filter((f) => !f.endsWith('storyboard.png')).slice(0, GALLERY_MAX);
	out.push(
		`#### ${screens.length} screen${screens.length === 1 ? '' : 's'} changed`,
		'',
		...shown.map((f) => {
			const [, , journey, variant, file] = f.split('/');
			return `<img src="https://raw.githubusercontent.com/${repo}/${sha}/${f}" width="380" alt="${journey} ${variant} ${file}">`;
		})
	);
	if (screens.length > shown.length) {
		out.push('', `…and ${screens.length - shown.length} more in the diff.`);
	}
	out.push('');
}

if (stale.length) {
	out.push(
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
	);
}

if (out.length) console.log(out.join('\n'));
