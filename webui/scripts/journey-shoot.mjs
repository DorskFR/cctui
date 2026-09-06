// Capture the screens for some or all journeys, then compress them.
//
//   npm run journey:shoot -- --all
//   npm run journey:shoot -- --changed          # journeys your edits can affect
//   npm run journey:shoot -- sessions-list
//
// Needs a seeded local stack (`make local/demo`). One book pass runs per theme:
// the theme lives in the server's settings blob rather than in the browser, so
// it has to be re-seeded between passes and cannot vary within one.
import { execFileSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { changedFiles, journeysForChanges, knownJourneys } from './journeys-for-changes.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const webui = resolve(here, '..');
const seed = resolve(webui, '../deploy/local/fixture/seed.sh');

const argv = process.argv.slice(2);
const token = process.env.CCTUI_TOKEN ?? 'dev-admin';
const env = { ...process.env, CCTUI_TOKEN: token, JOURNEY_VARS: JSON.stringify({ token }) };
const run = (cmd, args) => execFileSync(cmd, args, { cwd: webui, env, stdio: 'inherit' });

const compiled = JSON.parse(
	execFileSync('npx', ['journey', 'compile', '--public'], { cwd: webui, env, encoding: 'utf8' })
);

let ids;
if (argv.includes('--all')) ids = compiled.map((j) => j.id);
else if (argv.includes('--changed')) ids = journeysForChanges(changedFiles('origin/main'));
else ids = argv.filter((a) => !a.startsWith('--'));

if (!ids.length) {
	console.log('journey:shoot: nothing to capture');
	process.exit(0);
}

const unknown = ids.filter((id) => !knownJourneys().includes(id));
if (unknown.length) {
	console.error(`journey:shoot: unknown journey ${unknown.join(', ')}`);
	process.exit(1);
}

const byTheme = new Map();
for (const id of ids) {
	const ir = compiled.find((j) => j.id === id);
	for (const theme of ir.variants?.theme ?? ['dark']) {
		byTheme.set(theme, [...(byTheme.get(theme) ?? []), id]);
	}
}

// Unconditional: `vite preview` serves whatever is on disk, so a server left
// running from an earlier build would otherwise capture stale UI.
run('npm', ['run', 'build']);
run('node', [resolve(here, 'journey-auth.mjs')]);
for (const [theme, themeIds] of byTheme) {
	console.log(`\njourney:shoot: ${theme} — ${themeIds.join(', ')}`);
	run('bash', [seed, theme]);
	run('npx', ['journey', 'book', ...themeIds, '--variant', `theme=${theme}`]);
}
run('node', [resolve(here, 'journey-compress.mjs')]);
