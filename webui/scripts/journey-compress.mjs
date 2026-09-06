// Palette-quantise every captured screen. Playwright writes 24-bit PNGs of a
// flat-coloured UI, which is several times larger than the same image needs to
// be; the record is committed on every UI change, so the bytes matter.
//
//   node scripts/journey-compress.mjs [dir]
//
// A file is only rewritten when the result is actually smaller, which keeps the
// pass idempotent — running it twice does not re-encode what it already shrank.
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import sharp from 'sharp';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(process.argv[2] ?? join(here, '../../docs/journeys'));

function* pngs(dir) {
	for (const entry of readdirSync(dir, { withFileTypes: true })) {
		if (entry.name.startsWith('.')) continue;
		const path = join(dir, entry.name);
		if (entry.isDirectory()) yield* pngs(path);
		else if (entry.name.endsWith('.png')) yield path;
	}
}

const kb = (n) => `${(n / 1024).toFixed(0)} kB`;
let before = 0;
let after = 0;
let shrunk = 0;

for (const path of pngs(root)) {
	const original = readFileSync(path);
	const out = await sharp(original)
		.png({ palette: true, quality: 80, effort: 10 })
		.toBuffer();
	before += original.length;
	if (out.length < original.length) {
		writeFileSync(path, out);
		after += out.length;
		shrunk += 1;
	} else {
		after += original.length;
	}
}

console.log(
	`journey:compress: ${shrunk} rewritten · ${kb(before)} → ${kb(after)} (${(100 - (after / before) * 100).toFixed(0)}% smaller)`
);
