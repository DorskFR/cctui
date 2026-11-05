<script lang="ts">
	// Sessions › Launch & profiles: the same profile list the launch form uses,
	// with inline editing, outside of any launch.
	import { useAccountPools, useAccounts, useAllAccountsUsage, useProfileActions, useProfiles } from '$lib/queries';
	import ProfileList from '$lib/components/organisms/spawn/ProfileList.svelte';
	import type { ProfileSpecForm } from '$lib/components/organisms/spawn/profiles';
	import { errMessage } from '$lib/api';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	const profiles = useProfiles();
	const actions = useProfileActions();
	const accounts = useAccounts();
	const pools = useAccountPools();
	const usage = useAllAccountsUsage();

	let selectedId = $state<string | null>(null);
	let oneOff = $state<ProfileSpecForm | null>(null);
	let busy = $state(false);

	async function run<T>(work: () => Promise<T>, ok?: string): Promise<T | null> {
		busy = true;
		try {
			const out = await work();
			if (ok) toasts.ok(ok);
			return out;
		} catch (e) {
			toasts.error(m.spawn_profile_toast_failed({ error: errMessage(e) }));
			return null;
		} finally {
			busy = false;
		}
	}

	async function create(name: string, spec: ProfileSpecForm): Promise<boolean> {
		const p = await run(() => actions.create({ name, ...spec }));
		if (!p) return false;
		selectedId = p.id;
		oneOff = null;
		return true;
	}
</script>

<div class="profiles" data-setting-row data-journey="profiles-editor">
	<ProfileList
		profiles={profiles.data ?? []}
		bind:selectedId
		bind:oneOff
		accounts={accounts.data ?? []}
		pools={pools.data ?? []}
		usage={usage.data ?? []}
		machineId=""
		{busy}
		oncreate={create}
		onsave={(id, name, spec) => void run(() => actions.update(id, { name, spec }), m.spawn_profile_toast_saved())}
		ondelete={(id) =>
			void run(async () => {
				await actions.remove(id);
				if (selectedId === id) selectedId = null;
			})}
	/>
</div>

<style>
	.profiles {
		padding: var(--sp-3) var(--sp-4);
	}
</style>
