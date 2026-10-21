// Serve the Silero VAD model, its worklet and the onnxruntime wasm from
// `/vad/` ourselves: `@ricky0123/vad-web` would otherwise fetch them from a
// CDN at runtime.
import { copyFileSync, existsSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'static', 'vad');
const sources = [
	{ dir: join(root, 'node_modules', '@ricky0123', 'vad-web', 'dist'), keep: (f) => /\.(onnx|js)$/.test(f) },
	{ dir: join(root, 'node_modules', 'onnxruntime-web', 'dist'), keep: (f) => /\.(wasm|mjs)$/.test(f) }
];

const copied = [];
for (const { dir, keep } of sources) {
	if (!existsSync(dir)) {
		console.warn(`copy-vad-assets: ${dir} is missing (run npm ci); voice mode will not load`);
		continue;
	}
	mkdirSync(out, { recursive: true });
	for (const f of readdirSync(dir).filter(keep)) {
		copyFileSync(join(dir, f), join(out, f));
		copied.push(f);
	}
}
// Lets a deploy smoke check fetch every file and assert its content type.
if (copied.length) writeFileSync(join(out, 'manifest.json'), JSON.stringify(copied.sort(), null, '\t') + '\n');
console.log(`copy-vad-assets: ${copied.length} file(s) into static/vad/`);
