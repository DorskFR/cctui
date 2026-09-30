import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

/** Load `fixtures/parity/<name>.json`, the same case table
 *  `cargo test -p cctui-clientcore` replays against the Rust ports.
 *
 *  `import.meta.url` is an http: URL under Vite (and under happy-dom), so the
 *  repo root is located by walking up from the working directory instead. */
export function parityFixture<T>(name: string): T {
	const rel = `fixtures/parity/${name}.json`;
	let dir = process.cwd();
	for (;;) {
		const candidate = resolve(dir, rel);
		if (existsSync(candidate)) return JSON.parse(readFileSync(candidate, 'utf8')) as T;
		const parent = dirname(dir);
		if (parent === dir) throw new Error(`cannot locate ${rel} upward from ${process.cwd()}`);
		dir = parent;
	}
}
