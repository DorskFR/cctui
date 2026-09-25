import { describe, expect, it } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

function walk(dir: string, out: string[] = []): string[] {
	for (const e of readdirSync(dir)) {
		const p = join(dir, e);
		if (statSync(p).isDirectory()) walk(p, out);
		else if (/\.(svelte|ts)$/.test(e) && !/\.test\.ts$/.test(e)) out.push(p);
	}
	return out;
}

describe('query keys come from qk', () => {
	it('no source file outside keys.ts spells a literal query key', () => {
		const root = join(process.cwd(), 'src');
		const offenders = walk(root).filter(
			(f) => !f.endsWith(join('queries', 'keys.ts')) && /queryKey: \[['"]/.test(readFileSync(f, 'utf8'))
		);
		expect(offenders.map((f) => f.replace(root, 'src'))).toEqual([]);
	});
});
