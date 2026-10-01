#!/usr/bin/env node
// Splits the compiled public IR into one `docs/journeys/<id>/ir.json` per
// journey. Those files are committed: they are the spec the Rust TUI runner
// replays, and `cargo test` cannot run the compiler.
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const webui = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const docs = resolve(webui, '..', 'docs', 'journeys');

const bin = join(webui, 'node_modules', '.bin', 'journey');
const compiled = JSON.parse(execFileSync(bin, ['compile', '--public'], { cwd: webui, encoding: 'utf8' }));

const PUBLIC_JOURNEYS = publicIdsFromSource(join(webui, 'src', 'lib', 'journey.ts'));

/** `journey.ts` is Svelte-flavoured TS, so the list is read as text rather than imported. */
function publicIdsFromSource(file) {
	const src = readFileSync(file, 'utf8');
	const block = /PUBLIC_JOURNEYS: readonly string\[\] = \[([^\]]*)\]/.exec(src);
	if (!block) throw new Error('PUBLIC_JOURNEYS not found in journey.ts');
	return [...block[1].matchAll(/'([^']+)'/g)].map((m) => m[1]);
}

const ids = new Set(PUBLIC_JOURNEYS);
let written = 0;
for (const ir of compiled) {
	if (!ids.has(ir.id) || ir.steps.length === 0) continue;
	const dir = join(docs, ir.id);
	mkdirSync(dir, { recursive: true });
	writeFileSync(join(dir, 'ir.json'), `${JSON.stringify(ir, null, '\t')}\n`);
	written += 1;
}
console.log(`wrote ${written} ir.json under docs/journeys`);
