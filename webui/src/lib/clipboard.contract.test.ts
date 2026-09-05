import { describe, expect, it } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

// `navigator.clipboard` is undefined outside a secure context, so a raw call
// throws on any plain-http origin (a LAN dev server, an internal host). Every
// copy must go through the guarded helper, which falls back to execCommand.
function walk(dir: string, out: string[] = []): string[] {
	for (const e of readdirSync(dir)) {
		const p = join(dir, e);
		if (statSync(p).isDirectory()) walk(p, out);
		else if (/\.(svelte|ts)$/.test(e)) out.push(p);
	}
	return out;
}

describe('clipboard writes are secure-context safe', () => {
	it('no source file calls navigator.clipboard directly', () => {
		const root = join(process.cwd(), 'src');
		const offenders = walk(root).filter((f) => {
			if (f.endsWith('clipboard.ts') || f.endsWith('clipboard.contract.test.ts')) return false;
			return /navigator\.clipboard/.test(readFileSync(f, 'utf8'));
		});
		expect(offenders.map((f) => f.replace(root, 'src'))).toEqual([]);
	});
});
