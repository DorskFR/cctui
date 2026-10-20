import { describe, expect, it } from 'vitest';
import type { ContextItem } from '@bindings/ContextItem';
import { asAttached, asItems, asResolved, asSessionContext, newerVersion, resolveParams } from './context';

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

describe('newerVersion', () => {
	const delivered = { id: 'i1', kind: 'memory', name: 'n', title: 't', version: 2 };
	const at = (version: number) => [{ id: 'i1', version } as ContextItem];

	it('names the current version only when it moved past the delivered one', () => {
		expect(newerVersion(delivered, at(4))).toBe(4);
		expect(newerVersion(delivered, at(2))).toBeNull();
		expect(newerVersion(delivered, at(1))).toBeNull();
	});

	it('says nothing when the item is unknown or the list is not loaded', () => {
		expect(newerVersion(delivered, undefined)).toBeNull();
		expect(newerVersion(delivered, [])).toBeNull();
		expect(newerVersion({ ...delivered, id: 'gone' }, at(9))).toBeNull();
	});
});
