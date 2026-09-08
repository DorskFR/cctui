// Palette-quantise freshly captured screens. Playwright writes 24-bit PNGs of a
// flat-coloured UI, which is several times larger than the same image needs to
// be; the record is committed on every UI change, so the bytes matter.
//
//   node scripts/journey-compress.mjs <dir>...
//
// Quantisation is LOSSY and not detectable after the fact — a compressed PNG
// reads back like any other, so re-running this over the committed record
// re-quantises it and loses a little more colour every pass (measured: ~5% more
// bytes gone on a second run, which is degradation, not gain). It must run
// exactly once per capture, over the screens just written.
//
// That is why the directories are required rather than defaulting to the whole
// record: `journey:shoot` passes only the journeys it captured, so a shoot
// neither degrades nor dirties the flows it did not re-render.
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import sharp from 'sharp';

const roots = process.argv.slice(2).map((d) => resolve(d));
if (!roots.length) {
	console.error('journey:compress: needs the directories to compress, e.g. docs/journeys/<id>');
	console.error('  (it is lossy and must not be re-run over the whole record — see the header)');
	process.exit(1);
}

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

for (const root of roots) {
	if (!existsSync(root)) continue;
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
}

const saved = before ? (100 - (after / before) * 100).toFixed(0) : '0';
console.log(
	`journey:compress: ${shrunk} rewritten · ${kb(before)} → ${kb(after)} (${saved}% smaller)`
);
