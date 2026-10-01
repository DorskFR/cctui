import { describe, expect, it } from 'vitest';
import {
	recommended,
	switchOptions,
	type SwitchBinding,
	type SwitchCredential
} from '$lib/accountSwitch';
import { parityFixture } from './fixtures';

type Row = {
	accountName: string;
	pct: number | null;
	resetsInSecs: number | null;
	current: boolean;
	limited: boolean;
};

type Fixture = {
	switchOptions: {
		name: string;
		binding: SwitchBinding;
		credentials: SwitchCredential[];
		out: Row[];
		recommended: number | null;
	}[];
};

const fx = parityFixture<Fixture>('account_switch');

describe('accountSwitch parity fixtures', () => {
	it('switchOptions', () => {
		for (const c of fx.switchOptions) {
			const rows = switchOptions(c.binding, c.credentials);
			expect(
				rows.map((r) => ({
					accountName: r.accountName,
					pct: r.pct,
					resetsInSecs: r.resetsInSecs,
					current: r.current,
					limited: r.limited
				})),
				c.name
			).toEqual(c.out);
			expect(recommended(rows), c.name).toBe(c.recommended);
		}
	});
});
