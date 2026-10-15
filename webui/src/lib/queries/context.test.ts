import { describe, expect, it } from 'vitest';
import { asAttached, asItems, asResolved, asSessionContext, resolveParams } from './context';

describe('resolveParams', () => {
	it('drops empty coordinates and joins sorted label ids', () => {
		expect(
			resolveParams({ machine_id: 'm1', working_dir: '  /w/repo ', labels: ['b', 'a'], adapter: 'codex' })
		).toEqual({ machine_id: 'm1', working_dir: '/w/repo', labels: 'a,b', adapter: 'codex' });
		expect(resolveParams({ machine_id: 'm1', working_dir: ' ', labels: [], adapter: '' })).toEqual({
			machine_id: 'm1'
		});
	});
});

describe('response coercion', () => {
	it('turns a missing or foreign body into empty lists', () => {
		for (const body of [undefined, null, [], {}, 'nope', { rules: 3 }]) {
			expect(asResolved(body)).toEqual({ rules: [], memories: [] });
			expect(asSessionContext(body)).toEqual({ rules: [], memories: [] });
			expect(asAttached(body)).toEqual({ attached: [], unknown: [] });
		}
		expect(asItems({})).toEqual([]);
	});

	it('keeps well-formed items and fills in missing tags', () => {
		const item = { id: 'i1', name: 'n', title: 't' };
		expect(asItems([item, null, 'x'])).toEqual([{ ...item, tags: [] }]);
		expect(asResolved({ rules: [item], memories: [] }).rules).toHaveLength(1);
		expect(asSessionContext({ rules: [], memories: [{ id: 'm' }] }).memories).toEqual([{ id: 'm' }]);
	});
});
