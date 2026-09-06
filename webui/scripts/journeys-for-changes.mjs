// Which journeys a set of changed files can affect. Used by `journey:shoot
// --changed` locally and by the stale-screens CI comment; it deliberately
// depends on nothing but node, so CI can run it without installing.
//
//   node scripts/journeys-for-changes.mjs                    # vs origin/main
//   node scripts/journeys-for-changes.mjs --base HEAD~1
//   node scripts/journeys-for-changes.mjs --files a.svelte b.css
//
// Anything under webui/src that no rule claims maps to every journey. A shared
// token or a new common component must not quietly leave the record stale, and
// a rule that is merely missing should over-shoot rather than under-shoot.
import { execFileSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const journeysDir = resolve(here, '../journeys');

export function knownJourneys() {
	return readdirSync(journeysDir)
		.filter((f) => f.endsWith('.journey.ts'))
		.map((f) => f.replace(/\.journey\.ts$/, ''))
		.sort();
}

const SESSIONS = ['sessions-list', 'follow-session', 'search-sessions', 'spawn-session'];

// First match wins, so the narrow rules come before the catch-all.
const RULES = [
	{ match: /^webui\/journeys\/([^/]+)\.journey\.ts$/, journeys: (m) => [m[1]] },
	{ match: /^webui\/src\/routes\/sessions\//, journeys: () => SESSIONS },
	{ match: /^webui\/src\/routes\/access\//, journeys: () => ['enroll-machine'] },
	{ match: /^webui\/src\/routes\/settings\//, journeys: () => ['settings-tour'] },
	{ match: /^webui\/src\/routes\/\+page\.svelte$/, journeys: () => ['usage-overview'] },
	{ match: /^webui\/src\/lib\/components\/organisms\/(conversation|sessioncard|spawn)\//, journeys: () => SESSIONS },
	{ match: /^webui\/src\/lib\/components\/organisms\/access\//, journeys: () => ['enroll-machine'] },
	{ match: /^webui\/src\/lib\/components\/organisms\/settings\//, journeys: () => ['settings-tour'] },
	{ match: /^webui\/src\/lib\/components\/organisms\/EnrollMachineCard\.svelte$/, journeys: () => ['enroll-machine'] },
	{
		match: /^webui\/src\/lib\/components\/organisms\/(SessionCard|SessionControls|SpawnModal|ConversationDrawer)\.svelte$/,
		journeys: () => SESSIONS
	}
];

const IGNORED = [
	/^webui\/src\/.*\.test\.ts$/,
	/^webui\/src\/lib\/bindings\//,
	/^webui\/src\/lib\/paraglide\//
];

export function journeysForChanges(files, all = knownJourneys()) {
	const hit = new Set();
	for (const file of files) {
		if (!file.startsWith('webui/')) continue;
		if (IGNORED.some((re) => re.test(file))) continue;
		if (!/^webui\/(src|journeys)\//.test(file)) continue;
		const rule = RULES.find((r) => r.match.test(file));
		if (!rule) {
			return [...all];
		}
		for (const id of rule.journeys(file.match(rule.match))) hit.add(id);
	}
	return [...hit].filter((id) => all.includes(id)).sort();
}

export function changedFiles(base) {
	const git = (args) => execFileSync('git', args, { encoding: 'utf8' }).split('\n').filter(Boolean);
	const tracked = git(['diff', '--name-only', base, '--']);
	const dirty = git(['status', '--porcelain']).map((l) => l.slice(3).trim().split(' -> ').pop());
	return [...new Set([...tracked, ...dirty])];
}

if (import.meta.url === `file://${process.argv[1]}`) {
	const argv = process.argv.slice(2);
	const filesFlag = argv.indexOf('--files');
	const baseFlag = argv.indexOf('--base');
	const files =
		filesFlag >= 0
			? argv.slice(filesFlag + 1).filter((a) => !a.startsWith('--'))
			: changedFiles(baseFlag >= 0 ? argv[baseFlag + 1] : 'origin/main');
	const ids = journeysForChanges(files);
	if (argv.includes('--json')) console.log(JSON.stringify(ids));
	else for (const id of ids) console.log(id);
}
