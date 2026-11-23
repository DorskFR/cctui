// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';

const seeds = vi.hoisted(() => [] as unknown[]);
vi.mock('@dorsk/tsumikit', () => ({
	Avatar: (_anchor: unknown, props: { seed?: unknown }) => {
		seeds.push(props.seed);
	}
}));

const { default: AccountAvatar } = await import('./AccountAvatar.svelte');

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	seeds.length = 0;
});

const seedOf = (props: Record<string, unknown>) => {
	if (comp) unmount(comp);
	comp = mount(AccountAvatar, { target: document.body, props });
	return seeds.at(-1);
};

describe('AccountAvatar tint', () => {
	it('tints the same account identically wherever it is drawn', () => {
		const a = seedOf({ name: 'dorsk', id: 'acc-1', size: 20 });
		const b = seedOf({ name: 'other name', id: 'acc-1', size: 40 });
		expect(a).toBe('acc-1');
		expect(b).toBe(a);
	});

	it('falls back to the name when the account has no id', () => {
		expect(seedOf({ name: 'dorsk' })).toBe('dorsk');
	});
});
