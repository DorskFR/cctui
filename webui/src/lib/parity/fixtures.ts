import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/** Load `fixtures/parity/<name>.json` from the repo root. Read off disk rather
 *  than imported, because the directory sits outside the vite project root and
 *  is also read by `cargo test -p cctui-clientcore`. */
export function parityFixture<T>(name: string): T {
	const here = fileURLToPath(new URL('.', import.meta.url));
	return JSON.parse(readFileSync(`${here}../../../../fixtures/parity/${name}.json`, 'utf8')) as T;
}
