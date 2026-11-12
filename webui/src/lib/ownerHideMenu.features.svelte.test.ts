// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MeResponse } from '@bindings/MeResponse';
import type { SessionListItem } from '@bindings/SessionListItem';
import { setCapabilitiesForTest } from '$lib/features.svelte';
import { useOwnerHideMenu } from './ownerHideMenu.svelte';

const machinesEnabled = vi.hoisted(() => [] as boolean[]);

vi.mock('$lib/queries', () => ({
	useHiddenOwners: () => ({ data: { owners: [] } }),
	useAllMachines: (enabled: () => boolean) => {
		machinesEnabled.push(enabled());
		return { data: undefined };
	},
	useUsers: () => ({ data: [] }),
	useHiddenOwnerActions: () => ({ hide: vi.fn(), unhide: vi.fn() })
}));

const admin = { user_id: 'u-admin', role: 'admin' } as MeResponse;
const session = { id: 's1', machine_id: 'm1', machine_name: 'box' } as SessionListItem;
let stop: (() => void) | undefined;

function build() {
	let menu!: ReturnType<typeof useOwnerHideMenu>;
	stop = $effect.root(() => {
		menu = useOwnerHideMenu(
			() => session,
			() => admin
		);
	});
	flushSync();
	return menu;
}

afterEach(() => {
	stop?.();
	stop = undefined;
	machinesEnabled.length = 0;
	setCapabilitiesForTest(null);
});

describe('owner hide menu and machine spawn', () => {
	it('lists no machines without machine spawn, and still offers to hide the machine', () => {
		setCapabilitiesForTest({ features: [], switchable: ['machine_spawn'] });
		const menu = build();
		expect(machinesEnabled.every((e) => e === false)).toBe(true);
		expect(menu.items).toHaveLength(1);
	});

	it('lists machines for an admin with machine spawn on', () => {
		setCapabilitiesForTest({ features: ['machine_spawn'], switchable: ['machine_spawn'] });
		build();
		expect(machinesEnabled).toContain(true);
	});
});
