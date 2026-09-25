// Runs a codegen step only when its output is missing or older than an input,
// so `npm test`, `check` and `build` stay cheap after `npm ci` (whose `prepare`
// already ran `npm run codegen`) yet still work on a fresh or edited checkout.
import { execSync } from 'node:child_process';
import { existsSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

export const STEPS = [
	{
		script: 'paraglide',
		output: 'src/lib/paraglide/runtime.js',
		inputs: ['messages', 'project.inlang/settings.json']
	},
	{
		script: 'journey:compile',
		output: 'src/lib/journeys.generated.json',
		inputs: ['journeys']
	}
];

function newestMtime(path) {
	if (!existsSync(path)) return 0;
	const st = statSync(path);
	if (!st.isDirectory()) return st.mtimeMs;
	let newest = 0;
	for (const entry of readdirSync(path)) newest = Math.max(newest, newestMtime(join(path, entry)));
	return newest;
}

export function isStale(step, dir = root) {
	const out = join(dir, step.output);
	if (!existsSync(out)) return true;
	const built = statSync(out).mtimeMs;
	return step.inputs.some((input) => newestMtime(join(dir, input)) > built);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	for (const step of STEPS) {
		if (isStale(step)) execSync(`npm run --silent ${step.script}`, { cwd: root, stdio: 'inherit' });
	}
}
