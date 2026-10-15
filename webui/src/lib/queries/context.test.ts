import { describe, expect, it } from 'vitest';
import { resolveParams } from './context';

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
