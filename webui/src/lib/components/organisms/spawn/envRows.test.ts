import { describe, expect, it } from 'vitest';
import { badEnvKeys, envEdit, envMap, envNames, keepsStoredValue, storedEnvRows } from './envRows';

describe('env rows', () => {
	it('masks stored names: rows carry the name only', () => {
		expect(storedEnvRows(['A', 'B'])).toEqual([
			{ key: 'A', value: '', stored: true },
			{ key: 'B', value: '', stored: true }
		]);
	});

	it('round-trips keep, replace, remove and add into the write-only edit', () => {
		const rows = storedEnvRows(['KEEP', 'SWAP', 'GONE'])
			.filter((r) => r.key !== 'GONE')
			.map((r) => (r.key === 'SWAP' ? { ...r, value: 'new' } : r));
		rows.push({ key: ' ADDED ', value: 'v' }, { key: '', value: 'orphan' });
		expect(envEdit(rows)).toEqual({ env_keys: ['KEEP', 'SWAP', 'ADDED'], env: { SWAP: 'new', ADDED: 'v' } });
	});

	it('lists a name once and sends no value for an empty row', () => {
		expect(envNames([{ key: 'A', value: '' }, { key: ' A ', value: 'x' }])).toEqual(['A']);
		expect(envMap([{ key: 'A', value: '' }])).toEqual({});
	});

	it('knows when a request leans on a stored value', () => {
		expect(keepsStoredValue(storedEnvRows(['A']))).toBe(true);
		expect(keepsStoredValue([{ key: 'A', value: 'x', stored: true }])).toBe(false);
		expect(keepsStoredValue([{ key: 'A', value: '' }])).toBe(false);
	});

	it('flags names that are not shell variables', () => {
		expect(badEnvKeys([{ key: 'ok_no', value: '' }, { key: 'OK_1', value: '' }, { key: '', value: '' }])).toEqual([
			{ key: 'ok_no', value: '' }
		]);
	});
});
