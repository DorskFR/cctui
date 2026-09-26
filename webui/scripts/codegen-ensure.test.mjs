import { mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { isStale } from './codegen-ensure.mjs';

const step = { script: 'gen', output: 'out/generated.json', inputs: ['src', 'config.json'] };

let dir;
beforeEach(() => {
	dir = mkdtempSync(join(tmpdir(), 'codegen-ensure-'));
	mkdirSync(join(dir, 'src/nested'), { recursive: true });
	mkdirSync(join(dir, 'out'));
	writeFileSync(join(dir, 'src/nested/a.ts'), '');
	writeFileSync(join(dir, 'config.json'), '{}');
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

function touch(rel, seconds) {
	utimesSync(join(dir, rel), seconds, seconds);
}

describe('isStale', () => {
	it('regenerates when the output is missing', () => {
		expect(isStale(step, dir)).toBe(true);
	});

	it('skips when the output is newer than every input', () => {
		writeFileSync(join(dir, step.output), '{}');
		touch('src/nested/a.ts', 1000);
		touch('config.json', 1000);
		touch(step.output, 2000);
		expect(isStale(step, dir)).toBe(false);
	});

	it('regenerates when a nested input changed after the output', () => {
		writeFileSync(join(dir, step.output), '{}');
		touch('config.json', 1000);
		touch(step.output, 2000);
		touch('src/nested/a.ts', 3000);
		expect(isStale(step, dir)).toBe(true);
	});

	it('ignores inputs that do not exist', () => {
		writeFileSync(join(dir, step.output), '{}');
		touch('src/nested/a.ts', 1000);
		touch('config.json', 1000);
		touch(step.output, 2000);
		expect(isStale({ ...step, inputs: [...step.inputs, 'missing'] }, dir)).toBe(false);
	});
});
